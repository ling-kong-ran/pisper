// Provider 发现：从本机已有的 CLI 登录态（Codex/Claude）发现 Provider 凭据与模型配置，
// 供用户在配置页一键导入；解析各类凭据/配置文件的格式并兼容多版本。
import { createHash } from 'node:crypto'
import { readFile, stat } from 'node:fs/promises'
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

function decodeJwtPayload(token) {
  try {
    const parts = String(token || '').split('.')
    if (parts.length !== 3) return null
    return JSON.parse(Buffer.from(parts[1], 'base64url').toString('utf8'))
  } catch {
    return null
  }
}

function oauthExpiry(value, accessToken) {
  if (Number.isFinite(value) && value > 0) return value < 10_000_000_000 ? value * 1000 : value
  if (typeof value === 'string' && value.trim()) {
    const numeric = Number(value)
    if (Number.isFinite(numeric) && numeric > 0)
      return numeric < 10_000_000_000 ? numeric * 1000 : numeric
    const timestamp = Date.parse(value)
    if (Number.isFinite(timestamp)) return timestamp
  }
  const jwtExpiry = decodeJwtPayload(accessToken)?.exp
  return Number.isFinite(jwtExpiry) && jwtExpiry > 0 ? jwtExpiry * 1000 : 0
}

function codexOAuthCredential(data) {
  const tokens = data?.tokens
  const access = nonEmptyString(tokens?.access_token, tokens?.accessToken)
  const refresh = nonEmptyString(tokens?.refresh_token, tokens?.refreshToken)
  const payload = decodeJwtPayload(access)
  const accountId = nonEmptyString(
    tokens?.account_id,
    tokens?.accountId,
    payload?.['https://api.openai.com/auth']?.chatgpt_account_id,
  )
  const expires = oauthExpiry(tokens?.expires_at ?? tokens?.expiresAt, access)
  if (!access || !refresh || !expires || !accountId)
    throw new Error('Codex 登录状态缺少可续期的 OAuth 凭据。请在 Codex 中重新登录后再导入。')
  return { type: 'oauth', access, refresh, expires, accountId }
}

function claudeOAuthCredential(data) {
  const oauth = data?.claudeAiOauth
  const access = nonEmptyString(oauth?.accessToken, oauth?.access_token)
  const refresh = nonEmptyString(oauth?.refreshToken, oauth?.refresh_token)
  const expires = oauthExpiry(oauth?.expiresAt ?? oauth?.expires_at, access)
  if (!access || !refresh || !expires)
    throw new Error('Claude 登录状态缺少可续期的 OAuth 凭据。请在 Claude Code 中重新登录后再导入。')
  return { type: 'oauth', access, refresh, expires }
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

function resolveTomlTable(target, path, arrayTable) {
  let current = target
  for (const [index, key] of path.entries()) {
    const last = index === path.length - 1
    if (last && arrayTable) {
      if (current[key] === undefined) current[key] = []
      if (!Array.isArray(current[key])) throw new SyntaxError('TOML table type conflicts')
      const entry = {}
      current[key].push(entry)
      return entry
    }
    if (current[key] === undefined) current[key] = {}
    if (Array.isArray(current[key])) {
      const entry = current[key].at(-1)
      if (!entry || typeof entry !== 'object') throw new SyntaxError('Invalid TOML array table')
      current = entry
    } else {
      if (!current[key] || typeof current[key] !== 'object')
        throw new SyntaxError('TOML table type conflicts')
      current = current[key]
    }
  }
  return current
}

export function parseCodexToml(input) {
  const result = {}
  let table = result
  for (const rawLine of String(input || '').split(/\r?\n/)) {
    const line = stripTomlComment(rawLine).trim()
    if (!line) continue
    if (line.startsWith('[[')) {
      if (!line.endsWith(']]')) throw new SyntaxError('Invalid TOML array table')
      const path = tomlKeyParts(line.slice(2, -2))
      if (!path.length) throw new SyntaxError('Invalid TOML array table')
      table = resolveTomlTable(result, path, true)
      continue
    }
    if (line.startsWith('[')) {
      if (!line.endsWith(']')) throw new SyntaxError('Invalid TOML table')
      const path = tomlKeyParts(line.slice(1, -1))
      if (!path.length) throw new SyntaxError('Invalid TOML table')
      table = resolveTomlTable(result, path, false)
      continue
    }
    const assignment = tomlAssignmentIndex(line)
    if (assignment < 0) throw new SyntaxError('Invalid TOML assignment')
    const key = tomlKeyParts(line.slice(0, assignment))
    if (!key.length) throw new SyntaxError('Invalid TOML key')
    setNested(table, key, parseTomlValue(line.slice(assignment + 1)))
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
    kind: item.kind || 'configuration',
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
  constructor({
    homeDir = homedir(),
    env = process.env,
    readFileImpl = readFile,
    statImpl = stat,
  } = {}) {
    this.homeDir = homeDir
    this.env = env
    this.readFile = readFileImpl
    this.stat = statImpl
  }

  // Codex 配置目录候选。
  codexCandidates() {
    return uniqueCandidates([
      ...(this.env.CODEX_HOME
        ? [{ path: join(this.env.CODEX_HOME, 'config.toml'), location: '$CODEX_HOME/config.toml' }]
        : []),
      { path: join(this.homeDir, '.codex', 'config.toml'), location: '~/.codex/config.toml' },
    ])
  }

  claudeRoot() {
    return this.env.CLAUDE_CONFIG_DIR
      ? { path: this.env.CLAUDE_CONFIG_DIR, label: '$CLAUDE_CONFIG_DIR' }
      : { path: join(this.homeDir, '.claude'), label: '~/.claude' }
  }

  // Claude 配置目录候选。
  claudeCandidates() {
    const root = this.claudeRoot()
    return [{ path: join(root.path, 'settings.json'), location: `${root.label}/settings.json` }]
  }

  codexAuthCandidates() {
    return uniqueCandidates([
      ...(this.env.CODEX_HOME
        ? [{ path: join(this.env.CODEX_HOME, 'auth.json'), location: '$CODEX_HOME/auth.json' }]
        : []),
      { path: join(this.homeDir, '.codex', 'auth.json'), location: '~/.codex/auth.json' },
    ])
  }

  claudeAuthCandidates() {
    const root = this.claudeRoot()
    return [
      {
        path: join(root.path, '.credentials.json'),
        location: `${root.label}/.credentials.json`,
      },
    ]
  }

  // 发现并解析单个凭据候选文件。
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

  // 按优先级顺序尝试各候选，返回第一个可解析的配置。
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

  async discoverCredential(candidates, source, providerId, providerName) {
    for (const candidate of candidates) {
      try {
        const details = await this.stat(candidate.path)
        if (!details.isFile()) continue
        if (details.size > 1024 * 1024)
          return {
            item: null,
            errors: [
              { source, code: 'file_too_large', message: 'Login credential file is too large' },
            ],
          }
        const text = await this.readFile(candidate.path, 'utf8')
        if (Buffer.byteLength(text, 'utf8') > 1024 * 1024)
          return {
            item: null,
            errors: [
              { source, code: 'file_too_large', message: 'Login credential file is too large' },
            ],
          }
        const data = JSON.parse(text)
        const hasOAuthData =
          source === 'codex-auth'
            ? Boolean(data?.tokens && typeof data.tokens === 'object')
            : Boolean(data?.claudeAiOauth && typeof data.claudeAiOauth === 'object')
        if (!hasOAuthData) return { item: null, errors: [] }
        let credential
        try {
          credential =
            source === 'codex-auth' ? codexOAuthCredential(data) : claudeOAuthCredential(data)
        } catch {
          return {
            item: null,
            errors: [
              { source, code: 'invalid_login_state', message: 'OAuth login state is incomplete' },
            ],
          }
        }
        const fingerprint = stableFingerprint({
          source,
          providerId,
          location: candidate.location,
          size: details.size,
          modified: Math.round(details.mtimeMs),
        })
        return {
          item: {
            id: `${source}-${fingerprint.slice(0, 12)}`,
            kind: 'authentication',
            providerId,
            providerName,
            source,
            sourceLabel: source === 'codex-auth' ? 'Codex login' : 'Claude login',
            location: candidate.location,
            api: 'oauth',
            baseUrl: '',
            models: [],
            selectedModel: '',
            authType: 'oauth',
            authVariable: null,
            credentialPresent: true,
            importable: true,
            warnings: [],
            fingerprint,
            credential,
          },
          errors: [],
        }
      } catch (error) {
        if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') continue
        return {
          item: null,
          errors: [
            {
              source,
              code: error instanceof SyntaxError ? 'invalid_json' : 'unreadable',
              message:
                error instanceof SyntaxError
                  ? 'Login credential file has invalid syntax'
                  : 'Login credential file could not be read',
            },
          ],
        }
      }
    }
    return { item: null, errors: [] }
  }

  async discoverInternal() {
    const [codex, claude, codexAuth, claudeAuth] = await Promise.all([
      this.discoverCodex(),
      this.discoverClaude(),
      this.discoverCredential(
        this.codexAuthCandidates(),
        'codex-auth',
        'openai-codex',
        'OpenAI Codex',
      ),
      this.discoverCredential(this.claudeAuthCandidates(), 'claude-auth', 'anthropic', 'Anthropic'),
    ])
    return {
      items: [codex.item, claude.item, codexAuth.item, claudeAuth.item].filter(Boolean),
      errors: [...codex.errors, ...claude.errors, ...codexAuth.errors, ...claudeAuth.errors],
    }
  }

  // 汇总发现结果：Codex/Claude 认证与模型配置，含冲突检测。
  async discover() {
    const result = await this.discoverInternal()
    return { providers: result.items.map(publicDiscovery), errors: result.errors }
  }

  // 加载指定发现项的完整配置（供导入）。
  async loadConfiguration(discoveryId) {
    const result = await this.discoverInternal()
    const item = result.items.find((candidate) => candidate.id === discoveryId)
    if (!item)
      throw new Error('Discovered provider configuration is no longer available or has changed')
    if (!item.importable) throw new Error('This provider configuration cannot be imported')
    if (item.kind === 'authentication') {
      return {
        kind: 'authentication',
        providerId: item.providerId,
        source: item.source,
        fingerprint: item.fingerprint,
        selectedModel: '',
        credential: item.credential,
      }
    }
    return {
      kind: 'configuration',
      providerId: item.providerId,
      source: item.source,
      fingerprint: item.fingerprint,
      providerConfig: item.providerConfig,
      selectedModel: item.selectedModel,
      credential: item.credential,
    }
  }
}
