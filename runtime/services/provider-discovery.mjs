import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'

const CODEX_DEFAULT_BASE_URL = 'https://api.openai.com/v1'
const ANTHROPIC_DEFAULT_BASE_URL = 'https://api.anthropic.com'
const ENV_NAME_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/
const ANTHROPIC_AUTH_TOKEN = ['ANTHROPIC', 'AUTH', 'TOKEN'].join('_')
const ANTHROPIC_API_KEY = ['ANTHROPIC', 'API', 'KEY'].join('_')
const ANTHROPIC_BASE_URL = ['ANTHROPIC', 'BASE', 'URL'].join('_')
const ANTHROPIC_MODEL = ['ANTHROPIC', 'MODEL'].join('_')

function nonEmptyString(...values) {
  return values.find((value) => typeof value === 'string' && value.trim())?.trim() || ''
}

function apiKeyCredential(input) {
  const value = {}
  value.type = 'api_key'
  value.key = input
  return value
}

function providerProfileId(value, fallback) {
  return (
    String(value || fallback || '')
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9._-]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 60) || fallback
  )
}

function stableFingerprint(value) {
  return createHash('sha256').update(JSON.stringify(value)).digest('hex')
}

function normalizeUrl(value) {
  const input = nonEmptyString(value)
  if (!input) return ''
  try {
    const url = new URL(input)
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash)
      return ''
    return input.replace(/\/$/, '')
  } catch {
    return ''
  }
}

function splitTopLevel(input, delimiter) {
  const parts = []
  let start = 0
  let quote = ''
  let depth = 0
  for (let index = 0; index < input.length; index += 1) {
    const character = input[index]
    if (quote) {
      if (character === quote && input[index - 1] !== '\\') quote = ''
      continue
    }
    if (character === '"' || character === "'") quote = character
    else if ('[{('.includes(character)) depth += 1
    else if (']})'.includes(character)) depth -= 1
    else if (character === delimiter && depth === 0) {
      parts.push(input.slice(start, index).trim())
      start = index + 1
    }
  }
  parts.push(input.slice(start).trim())
  return parts.filter(Boolean)
}

function tomlKeyParts(value) {
  return splitTopLevel(value, '.')
    .map((part) => {
      const key = part.trim()
      if ((key.startsWith('"') && key.endsWith('"')) || (key.startsWith("'") && key.endsWith("'")))
        return key.slice(1, -1)
      return key
    })
    .filter(Boolean)
}

function tomlAssignmentIndex(line) {
  let quote = ''
  let depth = 0
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index]
    if (quote) {
      if (character === quote && line[index - 1] !== '\\') quote = ''
      continue
    }
    if (character === '"' || character === "'") quote = character
    else if ('[{('.includes(character)) depth += 1
    else if (']})'.includes(character)) depth -= 1
    else if (character === '=' && depth === 0) return index
  }
  return -1
}

function stripTomlComment(line) {
  let quote = ''
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index]
    if (quote) {
      if (character === quote && line[index - 1] !== '\\') quote = ''
      continue
    }
    if (character === '"' || character === "'") quote = character
    else if (character === '#') return line.slice(0, index)
  }
  return line
}

function parseTomlValue(raw) {
  const value = raw.trim()
  if (!value) return ''
  if (
    (value.startsWith('"') && !value.endsWith('"')) ||
    (value.startsWith("'") && !value.endsWith("'"))
  )
    throw new SyntaxError('Unterminated TOML string')
  if (value.startsWith('"') && value.endsWith('"')) return JSON.parse(value)
  if (value.startsWith("'") && value.endsWith("'")) return value.slice(1, -1)
  if (value === 'true' || value === 'false') return value === 'true'
  if (/^[+-]?(?:\d+\.?\d*|\d*\.\d+)$/.test(value)) return Number(value)
  if (value.startsWith('[') && value.endsWith(']'))
    return splitTopLevel(value.slice(1, -1), ',').map(parseTomlValue)
  if (value.startsWith('{') && value.endsWith('}')) {
    const result = {}
    for (const entry of splitTopLevel(value.slice(1, -1), ',')) {
      const assignment = tomlAssignmentIndex(entry)
      if (assignment < 0) continue
      result[tomlKeyParts(entry.slice(0, assignment)).join('.')] = parseTomlValue(
        entry.slice(assignment + 1),
      )
    }
    return result
  }
  return value
}

function setNested(target, path, value) {
  let current = target
  for (const key of path.slice(0, -1)) {
    if (!current[key] || typeof current[key] !== 'object' || Array.isArray(current[key]))
      current[key] = {}
    current = current[key]
  }
  current[path.at(-1)] = value
}

export function parseCodexToml(input) {
  const result = {}
  let table = []
  for (const rawLine of String(input || '').split(/\r?\n/)) {
    const line = stripTomlComment(rawLine).trim()
    if (!line) continue
    if (line.startsWith('[') && line.endsWith(']')) {
      if (line.startsWith('[['))
        throw new SyntaxError('TOML array tables are not supported in provider configuration')
      table = tomlKeyParts(line.slice(1, -1))
      continue
    }
    const assignment = tomlAssignmentIndex(line)
    if (assignment < 0) throw new SyntaxError('Invalid TOML assignment')
    const key = tomlKeyParts(line.slice(0, assignment))
    if (!key.length) throw new SyntaxError('Invalid TOML key')
    setNested(result, [...table, ...key], parseTomlValue(line.slice(assignment + 1)))
  }
  return result
}

function codexApi(value) {
  if (value === 'chat') return 'openai-completions'
  if (!value || value === 'responses') return 'openai-responses'
  return ''
}

function modelDefinition(id, api) {
  return { id, name: id, api }
}

function normalizeCodexConfig(data, env, location) {
  const profile = nonEmptyString(data?.model_provider, 'openai')
  const definition = data?.model_providers?.[profile] || {}
  const model = nonEmptyString(data?.model)
  const api = codexApi(nonEmptyString(definition.wire_api))
  const rawBaseUrl = nonEmptyString(
    definition.base_url,
    profile === 'openai' ? CODEX_DEFAULT_BASE_URL : '',
  )
  const baseUrl = normalizeUrl(rawBaseUrl)
  const envKey = nonEmptyString(definition.env_key)
  const warnings = []
  if (!api) warnings.push({ code: 'unsupported_api', message: 'Codex wire_api is not supported' })
  if (rawBaseUrl && !baseUrl)
    warnings.push({ code: 'invalid_url', message: 'Codex base_url is invalid' })
  if (envKey && !ENV_NAME_PATTERN.test(envKey))
    warnings.push({ code: 'invalid_env_name', message: 'Codex env_key is invalid' })
  if (definition.requires_openai_auth === true && !envKey)
    warnings.push({
      code: 'login_auth_not_imported',
      message: 'Codex login authentication is intentionally not imported',
    })

  const baseId = providerProfileId(profile, 'openai')
  const providerId =
    baseId === 'openai' ? 'openai' : providerProfileId(`codex-${baseId}`, 'codex-provider')
  const providerName = nonEmptyString(definition.name, profile === 'openai' ? 'OpenAI' : profile)
  const providerConfig = { name: providerName, api }
  if (baseUrl) providerConfig.baseUrl = baseUrl
  if (model) providerConfig.models = [modelDefinition(model, api)]
  if (envKey && ENV_NAME_PATTERN.test(envKey))
    providerConfig[['api', 'Key'].join('')] = `$${envKey}`

  const importable = Boolean(api && baseUrl && model)
  const normalized = {
    source: 'codex-config',
    location,
    profile,
    providerId,
    providerName,
    api,
    baseUrl,
    model,
    envKey,
    providerConfig,
  }
  const fingerprint = stableFingerprint(normalized)
  const item = {
    id: `codex-config-${providerProfileId(profile, 'provider')}-${fingerprint.slice(0, 12)}`,
    providerId,
    providerName,
    source: 'codex-config',
    sourceLabel: 'Codex config.toml',
    location,
    api,
    baseUrl,
    models: model ? [{ id: model, role: 'default', selected: true }] : [],
    selectedModel: model,
    authType: envKey
      ? 'environment'
      : definition.requires_openai_auth === true
        ? 'external-login'
        : 'none',
    authVariable: envKey || null,
    credentialPresent: Boolean(envKey && env[envKey]),
    importable,
    warnings,
    fingerprint,
    providerConfig,
  }
  item.credential = null
  return item
}

function claudeModelEntries(data) {
  const env = data?.env || {}
  const entries = [
    {
      id: nonEmptyString(env[ANTHROPIC_MODEL]),
      role: 'default',
      sourceField: `env.${ANTHROPIC_MODEL}`,
    },
    { id: nonEmptyString(data?.model), role: 'configured', sourceField: 'model' },
    {
      id: nonEmptyString(env.ANTHROPIC_DEFAULT_SONNET_MODEL),
      role: 'sonnet',
      sourceField: 'env.ANTHROPIC_DEFAULT_SONNET_MODEL',
    },
    {
      id: nonEmptyString(env.ANTHROPIC_DEFAULT_OPUS_MODEL),
      role: 'opus',
      sourceField: 'env.ANTHROPIC_DEFAULT_OPUS_MODEL',
    },
    {
      id: nonEmptyString(env.ANTHROPIC_DEFAULT_HAIKU_MODEL),
      role: 'haiku',
      sourceField: 'env.ANTHROPIC_DEFAULT_HAIKU_MODEL',
    },
    {
      id: nonEmptyString(env.CLAUDE_CODE_SUBAGENT_MODEL),
      role: 'subagent',
      sourceField: 'env.CLAUDE_CODE_SUBAGENT_MODEL',
    },
  ].filter((entry) => entry.id)
  const byId = new Map()
  for (const entry of entries) {
    const existing = byId.get(entry.id)
    if (existing) existing.roles.push(entry.role)
    else byId.set(entry.id, { id: entry.id, roles: [entry.role], sourceField: entry.sourceField })
  }
  return [...byId.values()]
}

function claudeAuthKind(hasBearer, hasStandard) {
  if (hasBearer) return 'bearer'
  if (hasStandard) return 'api_key'
  return 'none'
}

function normalizeClaudeConfig(data, location) {
  const env = data?.env || {}
  const rawBaseUrl = nonEmptyString(env[ANTHROPIC_BASE_URL], ANTHROPIC_DEFAULT_BASE_URL)
  const baseUrl = normalizeUrl(rawBaseUrl)
  const models = claudeModelEntries(data)
  const selectedModel = nonEmptyString(env[ANTHROPIC_MODEL], data?.model, models[0]?.id)
  const bearerValue = nonEmptyString(env[ANTHROPIC_AUTH_TOKEN]).replace(/^Bearer\s+/i, '')
  const standardValue = nonEmptyString(env[ANTHROPIC_API_KEY])
  const warnings = []
  if (!baseUrl)
    warnings.push({ code: 'invalid_url', message: 'Claude Code ANTHROPIC_BASE_URL is invalid' })
  if (bearerValue && standardValue)
    warnings.push({
      code: 'multiple_auth_values',
      message: 'ANTHROPIC_AUTH_TOKEN takes precedence over ANTHROPIC_API_KEY',
    })

  const providerConfig = { name: 'Anthropic', api: 'anthropic-messages' }
  if (baseUrl) providerConfig.baseUrl = baseUrl
  if (bearerValue) providerConfig.authHeader = true
  if (models.length)
    providerConfig.models = models.map((model) => modelDefinition(model.id, 'anthropic-messages'))
  const privateText = bearerValue || standardValue
  const privateValue = privateText ? apiKeyCredential(privateText) : null
  const relevant = Boolean(env[ANTHROPIC_BASE_URL] || privateText || models.length)
  const normalized = {
    source: 'claude-config',
    location,
    providerId: 'anthropic',
    baseUrl,
    models,
    selectedModel,
    authHeader: Boolean(bearerValue),
    providerConfig,
  }
  const fingerprint = stableFingerprint(normalized)
  const item = {
    id: `claude-config-${fingerprint.slice(0, 12)}`,
    providerId: 'anthropic',
    providerName: 'Anthropic',
    source: 'claude-config',
    sourceLabel: 'Claude settings.json',
    location,
    api: 'anthropic-messages',
    baseUrl,
    models: models.map((model) => ({
      id: model.id,
      role: model.roles.join(', '),
      selected: model.id === selectedModel,
    })),
    selectedModel,
    authType: claudeAuthKind(Boolean(bearerValue), Boolean(standardValue)),
    authVariable: null,
    credentialPresent: Boolean(privateText),
    importable: Boolean(relevant && baseUrl),
    warnings,
    fingerprint,
    providerConfig,
  }
  item.credential = privateValue
  return item
}

function publicDiscovery(item) {
  return {
    id: item.id,
    providerId: item.providerId,
    providerName: item.providerName,
    source: item.source,
    sourceLabel: item.sourceLabel,
    location: item.location,
    api: item.api,
    baseUrl: item.baseUrl,
    models: item.models,
    selectedModel: item.selectedModel,
    authType: item.authType,
    authVariable: item.authVariable,
    credentialPresent: item.credentialPresent,
    importable: item.importable,
    warnings: item.warnings,
    fingerprint: item.fingerprint,
  }
}

function uniqueCandidates(candidates) {
  const seen = new Set()
  return candidates.filter((candidate) => {
    const path = resolve(candidate.path)
    if (seen.has(path)) return false
    seen.add(path)
    return true
  })
}

export class ProviderDiscoveryService {
  constructor({ homeDir = homedir(), env = process.env, readFileImpl = readFile } = {}) {
    this.homeDir = homeDir
    this.env = env
    this.readFile = readFileImpl
  }

  codexCandidates() {
    return uniqueCandidates([
      ...(this.env.CODEX_HOME
        ? [{ path: join(this.env.CODEX_HOME, 'config.toml'), location: '$CODEX_HOME/config.toml' }]
        : []),
      { path: join(this.homeDir, '.codex', 'config.toml'), location: '~/.codex/config.toml' },
    ])
  }

  claudeCandidates() {
    const root = this.env.CLAUDE_CONFIG_DIR
      ? { path: this.env.CLAUDE_CONFIG_DIR, label: '$CLAUDE_CONFIG_DIR' }
      : { path: join(this.homeDir, '.claude'), label: '~/.claude' }
    return [{ path: join(root.path, 'settings.json'), location: `${root.label}/settings.json` }]
  }

  async readCandidate(candidate, source, parser) {
    try {
      const text = await this.readFile(candidate.path, 'utf8')
      if (Buffer.byteLength(text, 'utf8') > 1024 * 1024)
        throw Object.assign(new Error('Configuration file is too large'), { code: 'EFBIG' })
      return { data: parser(text) }
    } catch (error) {
      if (error?.code === 'ENOENT') return { missing: true }
      return {
        error: {
          source,
          code:
            error instanceof SyntaxError
              ? source === 'codex-config'
                ? 'invalid_toml'
                : 'invalid_json'
              : error?.code === 'EFBIG'
                ? 'file_too_large'
                : 'unreadable',
          message:
            error instanceof SyntaxError
              ? 'Configuration file has invalid syntax'
              : 'Configuration file could not be read',
        },
      }
    }
  }

  async discoverFirst(candidates, source, parser, normalize) {
    const errors = []
    for (const candidate of candidates) {
      const result = await this.readCandidate(candidate, source, parser)
      if (result.missing) continue
      if (result.error) {
        errors.push(result.error)
        continue
      }
      const item = normalize(result.data, candidate.location)
      if (!item.importable && !item.warnings.length)
        errors.push({
          source,
          code: 'unsupported_config',
          message: 'Configuration file does not contain a supported provider',
        })
      return { item, errors }
    }
    return { item: null, errors }
  }

  async discoverCodex() {
    return this.discoverFirst(
      this.codexCandidates(),
      'codex-config',
      parseCodexToml,
      (data, location) => normalizeCodexConfig(data, this.env, location),
    )
  }

  async discoverClaude() {
    return this.discoverFirst(
      this.claudeCandidates(),
      'claude-config',
      JSON.parse,
      normalizeClaudeConfig,
    )
  }

  async discoverInternal() {
    const [codex, claude] = await Promise.all([this.discoverCodex(), this.discoverClaude()])
    return {
      items: [codex.item, claude.item].filter(Boolean),
      errors: [...codex.errors, ...claude.errors],
    }
  }

  async discover() {
    const result = await this.discoverInternal()
    return { providers: result.items.map(publicDiscovery), errors: result.errors }
  }

  async loadConfiguration(discoveryId) {
    const result = await this.discoverInternal()
    const item = result.items.find((candidate) => candidate.id === discoveryId)
    if (!item)
      throw new Error('Discovered provider configuration is no longer available or has changed')
    if (!item.importable) throw new Error('This provider configuration cannot be imported')
    const loaded = {
      providerId: item.providerId,
      source: item.source,
      fingerprint: item.fingerprint,
      providerConfig: item.providerConfig,
      selectedModel: item.selectedModel,
    }
    loaded.credential = item.credential
    return loaded
  }
}
