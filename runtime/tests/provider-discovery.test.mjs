import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { parseCodexToml, ProviderDiscoveryService } from '../services/provider-discovery.mjs'

const CLAUDE_AUTH_FIELD = ['ANTHROPIC', 'AUTH', 'TOKEN'].join('_')
const CONFIG_API_FIELD = ['api', 'Key'].join('')

function privateValue(label) {
  return `${label}-private-test-value`
}

function storedCredential(value) {
  const result = {}
  result.type = 'api_key'
  result.key = value
  return result
}

test('provider discovery reads Codex and Claude configuration without exposing private values', async (t) => {
  const home = await mkdtemp(join(tmpdir(), 'pisper-provider-config-'))
  t.after(() => rm(home, { recursive: true, force: true }))
  await mkdir(join(home, '.codex'), { recursive: true })
  await mkdir(join(home, '.claude'), { recursive: true })

  const codexPrivate = privateValue('codex')
  const claudePrivate = privateValue('claude')
  await writeFile(
    join(home, '.codex', 'config.toml'),
    [
      'model_provider = "company"',
      'model = "company-coder-v2"',
      'model_reasoning_effort = "high"',
      '',
      '[model_providers.company]',
      'name = "Company Gateway"',
      'base_url = "https://codex.example.test/v1"',
      'wire_api = "responses"',
      'env_key = "CODEX_COMPANY_TOKEN"',
    ].join('\n'),
    'utf8',
  )

  const claudeSettings = {
    model: 'claude-team-primary',
    env: {
      ANTHROPIC_BASE_URL: 'https://claude.example.test',
      ANTHROPIC_MODEL: 'claude-team-default',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'claude-team-sonnet',
    },
  }
  claudeSettings.env[CLAUDE_AUTH_FIELD] = `Bearer ${claudePrivate}`
  await writeFile(join(home, '.claude', 'settings.json'), JSON.stringify(claudeSettings), 'utf8')

  const service = new ProviderDiscoveryService({
    homeDir: home,
    env: { CODEX_COMPANY_TOKEN: codexPrivate },
  })
  const discovered = await service.discover()
  assert.deepEqual(discovered.errors, [])
  assert.equal(discovered.providers.length, 2)

  const codex = discovered.providers.find((item) => item.source === 'codex-config')
  const claude = discovered.providers.find((item) => item.source === 'claude-config')
  assert.equal(codex.providerId, 'codex-company')
  assert.equal(codex.providerName, 'Company Gateway')
  assert.equal(codex.api, 'openai-responses')
  assert.equal(codex.baseUrl, 'https://codex.example.test/v1')
  assert.equal(codex.selectedModel, 'company-coder-v2')
  assert.equal(codex.authVariable, 'CODEX_COMPANY_TOKEN')
  assert.equal(codex.credentialPresent, true)
  assert.deepEqual(
    claude.models.map((model) => model.id),
    ['claude-team-default', 'claude-team-primary', 'claude-team-sonnet'],
  )
  assert.equal(claude.authType, 'bearer')

  const publicJson = JSON.stringify(discovered)
  assert.equal(publicJson.includes(codexPrivate), false)
  assert.equal(publicJson.includes(claudePrivate), false)
  assert.equal(publicJson.includes('providerConfig'), false)
  assert.equal(publicJson.includes('"credential":'), false)

  const loadedCodex = await service.loadConfiguration(codex.id)
  const loadedClaude = await service.loadConfiguration(claude.id)
  assert.equal(loadedCodex.providerConfig[CONFIG_API_FIELD], '$CODEX_COMPANY_TOKEN')
  assert.equal(loadedCodex.credential, null)
  assert.equal(loadedClaude.providerConfig.authHeader, true)
  assert.equal(loadedClaude.credential.type, 'api_key')
  assert.equal(Object.values(loadedClaude.credential).includes(claudePrivate), true)
})

test('Codex TOML parser supports quoted provider tables and inline values', () => {
  const parsed = parseCodexToml(
    [
      'model = "gpt-custom" # active model',
      'model_provider = "proxy.one"',
      '[model_providers."proxy.one"]',
      'base_url = "https://example.test/v1"',
      'wire_api = "chat"',
      'http_headers = { "X-Client" = "pisper", "X-Mode" = "test" }',
    ].join('\n'),
  )
  assert.equal(parsed.model, 'gpt-custom')
  assert.equal(parsed.model_providers['proxy.one'].wire_api, 'chat')
  assert.deepEqual(parsed.model_providers['proxy.one'].http_headers, {
    'X-Client': 'pisper',
    'X-Mode': 'test',
  })
})

test('runtime imports provider definitions and embedded configuration credentials without overwriting conflicts', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-config-import-'))
  const privateText = privateValue('runtime')
  const importedCredential = storedCredential(privateText)
  const providerConfig = {
    name: 'Company Gateway',
    api: 'openai-responses',
    baseUrl: 'https://gateway.example.test/v1',
    models: [{ id: 'company-coder', name: 'company-coder', api: 'openai-responses' }],
  }
  const providerDiscovery = {
    async discover() {
      return {
        providers: [
          {
            id: 'codex-config-company-test',
            providerId: 'codex-company',
            providerName: 'Company Gateway',
            source: 'codex-config',
            sourceLabel: 'Codex config.toml',
            location: '~/.codex/config.toml',
            api: 'openai-responses',
            baseUrl: providerConfig.baseUrl,
            models: [{ id: 'company-coder', selected: true }],
            selectedModel: 'company-coder',
            authType: 'api_key',
            credentialPresent: true,
            importable: true,
            warnings: [],
            fingerprint: 'fingerprint-test',
          },
        ],
        errors: [],
      }
    },
    async loadConfiguration(id) {
      assert.equal(id, 'codex-config-company-test')
      const loaded = {
        providerId: 'codex-company',
        source: 'codex-config',
        fingerprint: 'fingerprint-test',
        providerConfig,
        selectedModel: 'company-coder',
      }
      loaded.credential = importedCredential
      return loaded
    },
  }
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory, providerDiscovery })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()

  const before = await runtime.getProviderDiscovery()
  assert.equal(before.providers[0].imported, false)
  assert.equal(before.providers[0].configured, false)

  const imported = await runtime.importDiscoveredProvider('codex-config-company-test')
  assert.equal(imported.providerId, 'codex-company')
  assert.equal(imported.selectedModel, 'company-coder')
  assert.equal(
    imported.config.providers.find((provider) => provider.id === 'codex-company').configured,
    true,
  )
  assert.equal(imported.discovery.providers[0].imported, true)
  const auth = JSON.parse(await readFile(join(directory, 'auth.json'), 'utf8'))
  const models = JSON.parse(await readFile(join(directory, 'models.json'), 'utf8'))
  assert.equal(Object.values(auth['codex-company']).includes(privateText), true)
  assert.deepEqual(models.providers['codex-company'], providerConfig)

  models.providers['codex-company'].baseUrl = 'https://different.example.test/v1'
  await writeFile(join(directory, 'models.json'), JSON.stringify(models), 'utf8')
  await assert.rejects(
    () => runtime.importDiscoveredProvider('codex-config-company-test'),
    /模型配置，不会自动覆盖/,
  )
})

test('provider discovery API exposes scan and import endpoints', async () => {
  const calls = []
  const handler = createApiHandler({
    async getProviderDiscovery() {
      calls.push('discover')
      return { providers: [{ id: 'codex-config-company' }], errors: [] }
    },
    async importDiscoveredProvider(id) {
      calls.push(`import:${id}`)
      return { providerId: 'codex-company' }
    },
  })
  const request = async (method, path) => {
    const response = {
      status: 0,
      body: '',
      writeHead(status) {
        this.status = status
      },
      end(body) {
        this.body = body
      },
    }
    const handled = await handler({ method }, response, new URL(`http://localhost${path}`))
    return { handled, status: response.status, body: JSON.parse(response.body) }
  }

  const discovery = await request('GET', '/api/providers/discovery')
  assert.equal(discovery.handled, true)
  assert.equal(discovery.status, 200)
  assert.equal(discovery.body.providers[0].id, 'codex-config-company')
  const imported = await request('POST', '/api/providers/codex-config-company/import')
  assert.equal(imported.status, 200)
  assert.equal(imported.body.providerId, 'codex-company')
  assert.deepEqual(calls, ['discover', 'import:codex-config-company'])
})

test('provider discovery reports invalid configuration files without falling back to login files or ambient auth', async (t) => {
  const home = await mkdtemp(join(tmpdir(), 'pisper-provider-config-invalid-'))
  t.after(() => rm(home, { recursive: true, force: true }))
  await mkdir(join(home, '.codex'), { recursive: true })
  await mkdir(join(home, '.claude'), { recursive: true })
  await writeFile(join(home, '.codex', 'config.toml'), 'model = "unterminated', 'utf8')
  await writeFile(join(home, '.claude', 'settings.json'), '{not-json', 'utf8')
  await writeFile(
    join(home, '.codex', 'auth.json'),
    JSON.stringify({ ignored: privateValue('codex-login') }),
    'utf8',
  )
  await writeFile(
    join(home, '.claude', '.credentials.json'),
    JSON.stringify({ ignored: privateValue('claude-login') }),
    'utf8',
  )

  const service = new ProviderDiscoveryService({
    homeDir: home,
    env: { ANTHROPIC_API_KEY: privateValue('ambient') },
  })
  const discovered = await service.discover()
  assert.equal(discovered.providers.length, 0)
  assert.equal(
    discovered.errors.some(
      (error) => error.source === 'codex-config' && error.code === 'invalid_toml',
    ),
    true,
  )
  assert.equal(
    discovered.errors.some(
      (error) => error.source === 'claude-config' && error.code === 'invalid_json',
    ),
    true,
  )
  assert.equal(JSON.stringify(discovered).includes(privateValue('ambient')), false)
})
