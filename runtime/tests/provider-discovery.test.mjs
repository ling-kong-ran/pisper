import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
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

function fakeJwt(payload) {
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString('base64url')
  return `${encode({ alg: 'none', typ: 'JWT' })}.${encode(payload)}.test-signature`
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
      '',
      '[[skills.config]]',
      'path = "/Users/example/.codex/skills/company/SKILL.md"',
      'enabled = false',
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

test('Codex TOML parser accepts unrelated array tables from current desktop configurations', () => {
  const parsed = parseCodexToml(
    [
      'model_provider = "custom"',
      'model = "gpt-5.5"',
      '[model_providers.custom]',
      'base_url = "https://icode.example.test/v1"',
      'requires_openai_auth = true',
      'wire_api = "responses"',
      '[plugins."documents@openai-primary-runtime"]',
      'enabled = true',
      '[[skills.config]]',
      'path = "/Users/example/.codex/skills/one/SKILL.md"',
      'enabled = false',
      '[[skills.config]]',
      'path = "/Users/example/.codex/skills/two/SKILL.md"',
      'enabled = true',
    ].join('\n'),
  )

  assert.equal(parsed.model, 'gpt-5.5')
  assert.equal(parsed.model_providers.custom.wire_api, 'responses')
  assert.equal(parsed.plugins['documents@openai-primary-runtime'].enabled, true)
  assert.deepEqual(parsed.skills.config, [
    { path: '/Users/example/.codex/skills/one/SKILL.md', enabled: false },
    { path: '/Users/example/.codex/skills/two/SKILL.md', enabled: true },
  ])
})

test('OAuth login discovery stays secret until explicit import and targets official providers only', async (t) => {
  const home = await mkdtemp(join(tmpdir(), 'pisper-provider-oauth-'))
  t.after(() => rm(home, { recursive: true, force: true }))
  await mkdir(join(home, '.codex'), { recursive: true })
  await mkdir(join(home, '.claude'), { recursive: true })

  const codexAccess = fakeJwt({
    exp: Math.floor(Date.now() / 1000) + 3600,
    'https://api.openai.com/auth': { chatgpt_account_id: 'account-test' },
  })
  const codexRefresh = privateValue('codex-refresh')
  const claudeAccess = privateValue('claude-access')
  const claudeRefresh = privateValue('claude-refresh')
  const codexPath = join(home, '.codex', 'auth.json')
  const claudePath = join(home, '.claude', '.credentials.json')
  await writeFile(
    codexPath,
    JSON.stringify({
      OPENAI_API_KEY: null,
      tokens: { access_token: codexAccess, refresh_token: codexRefresh },
    }),
    'utf8',
  )
  await writeFile(
    claudePath,
    JSON.stringify({
      claudeAiOauth: {
        accessToken: claudeAccess,
        refreshToken: claudeRefresh,
        expiresAt: Date.now() + 3600_000,
      },
    }),
    'utf8',
  )

  const reads = []
  const service = new ProviderDiscoveryService({
    homeDir: home,
    env: {},
    readFileImpl: async (path, encoding) => {
      reads.push(path)
      return readFile(path, encoding)
    },
  })
  const discovered = await service.discover()
  assert.equal(reads.filter((path) => path === codexPath).length, 1)
  assert.equal(reads.filter((path) => path === claudePath).length, 1)
  assert.deepEqual(
    discovered.providers.map((item) => [item.source, item.providerId, item.kind]),
    [
      ['codex-auth', 'openai-codex', 'authentication'],
      ['claude-auth', 'anthropic', 'authentication'],
    ],
  )
  const publicJson = JSON.stringify(discovered)
  for (const secret of [codexAccess, codexRefresh, claudeAccess, claudeRefresh])
    assert.equal(publicJson.includes(secret), false)

  const codex = await service.loadConfiguration(discovered.providers[0].id)
  assert.equal(reads.filter((path) => path === codexPath).length, 2)
  assert.equal(reads.filter((path) => path === claudePath).length, 2)
  const claude = await service.loadConfiguration(discovered.providers[1].id)
  assert.equal(codex.kind, 'authentication')
  assert.equal(codex.providerId, 'openai-codex')
  assert.equal(codex.credential.type, 'oauth')
  assert.equal(codex.credential.accountId, 'account-test')
  assert.equal(codex.credential.refresh, codexRefresh)
  assert.equal(claude.kind, 'authentication')
  assert.equal(claude.providerId, 'anthropic')
  assert.equal(claude.credential.type, 'oauth')
  assert.equal(claude.credential.refresh, claudeRefresh)
  assert.equal(reads.filter((path) => path === codexPath).length, 3)
  assert.equal(reads.filter((path) => path === claudePath).length, 3)
})

test('OAuth import rescans and validates the current credential file', async (t) => {
  const home = await mkdtemp(join(tmpdir(), 'pisper-provider-oauth-rescan-'))
  t.after(() => rm(home, { recursive: true, force: true }))
  await mkdir(join(home, '.codex'), { recursive: true })
  const authPath = join(home, '.codex', 'auth.json')
  const access = fakeJwt({
    exp: Math.floor(Date.now() / 1000) + 3600,
    'https://api.openai.com/auth': { chatgpt_account_id: 'account-test' },
  })
  const auth = (refresh) =>
    JSON.stringify({ tokens: { access_token: access, refresh_token: refresh } })
  await writeFile(authPath, auth('refresh-before-scan'), 'utf8')

  const service = new ProviderDiscoveryService({
    homeDir: home,
    env: {},
    statImpl: async (path) => {
      const details = await stat(path)
      return path === authPath ? { isFile: () => details.isFile(), size: 100, mtimeMs: 1 } : details
    },
  })
  const discovered = await service.discover()
  const candidate = discovered.providers.find((provider) => provider.source === 'codex-auth')
  assert.ok(candidate)

  await writeFile(authPath, auth('refresh-after-scan'), 'utf8')
  const loaded = await service.loadConfiguration(candidate.id)
  assert.equal(loaded.credential.refresh, 'refresh-after-scan')

  await writeFile(authPath, JSON.stringify({ OPENAI_API_KEY: 'api-key-only' }), 'utf8')
  await assert.rejects(
    () => service.loadConfiguration(candidate.id),
    /no longer available or has changed/,
  )
})

test('runtime imports OAuth login without creating a custom Provider or overwriting auth', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-oauth-import-'))
  const credential = {
    type: 'oauth',
    access: privateValue('oauth-access'),
    refresh: privateValue('oauth-refresh'),
    expires: Date.now() + 3600_000,
    accountId: 'account-test',
  }
  const providerDiscovery = {
    async discover() {
      return {
        providers: [
          {
            id: 'codex-auth-test',
            kind: 'authentication',
            providerId: 'openai-codex',
            providerName: 'OpenAI Codex',
            source: 'codex-auth',
            location: '~/.codex/auth.json',
            authType: 'oauth',
            importable: true,
            fingerprint: 'oauth-fingerprint',
          },
        ],
        errors: [],
      }
    },
    async loadConfiguration(id) {
      assert.equal(id, 'codex-auth-test')
      return {
        kind: 'authentication',
        providerId: 'openai-codex',
        source: 'codex-auth',
        fingerprint: 'oauth-fingerprint',
        selectedModel: '',
        credential,
      }
    },
  }
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory, providerDiscovery })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()

  const imported = await runtime.importDiscoveredProvider('codex-auth-test')
  assert.equal(imported.kind, 'authentication')
  assert.equal(imported.providerId, 'openai-codex')
  assert.equal(imported.discovery.providers[0].imported, true)
  assert.equal(
    imported.config.providers.find((provider) => provider.id === 'openai-codex').configured,
    true,
  )
  const authPath = join(directory, 'auth.json')
  const auth = JSON.parse(await readFile(authPath, 'utf8'))
  assert.deepEqual(auth['openai-codex'], credential)
  if (process.platform !== 'win32') assert.equal((await stat(authPath)).mode & 0o777, 0o600)
  await assert.rejects(() => readFile(join(directory, 'models.json'), 'utf8'), /ENOENT/)

  auth['openai-codex'] = storedCredential(privateValue('existing-auth'))
  await writeFile(authPath, JSON.stringify(auth), 'utf8')
  await assert.rejects(
    () => runtime.importDiscoveredProvider('codex-auth-test'),
    /认证，不会自动覆盖/,
  )

  delete auth['openai-codex']
  await writeFile(authPath, JSON.stringify(auth), 'utf8')
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        'openai-codex': {
          baseUrl: 'https://relay.example.test/v1',
          models: [{ id: 'gpt-test', baseUrl: 'https://relay.example.test/v1' }],
        },
      },
    }),
    'utf8',
  )
  await assert.rejects(
    () => runtime.importDiscoveredProvider('codex-auth-test'),
    /避免登录凭据外发/,
  )
})

test('login discovery ignores API keys and rejects incomplete OAuth sessions', async (t) => {
  const home = await mkdtemp(join(tmpdir(), 'pisper-provider-oauth-invalid-'))
  t.after(() => rm(home, { recursive: true, force: true }))
  await mkdir(join(home, '.codex'), { recursive: true })
  await mkdir(join(home, '.claude'), { recursive: true })
  await writeFile(join(home, '.codex', 'auth.json'), JSON.stringify({ OPENAI_API_KEY: 'test' }))
  await writeFile(
    join(home, '.claude', '.credentials.json'),
    JSON.stringify({ claudeAiOauth: { accessToken: 'access-only' } }),
  )
  const service = new ProviderDiscoveryService({ homeDir: home, env: {} })
  const discovered = await service.discover()
  assert.equal(
    discovered.providers.some((provider) => provider.source === 'codex-auth'),
    false,
  )
  assert.equal(
    discovered.providers.some((provider) => provider.source === 'claude-auth'),
    false,
  )
  assert.deepEqual(discovered.errors, [
    {
      source: 'claude-auth',
      code: 'invalid_login_state',
      message: 'OAuth login state is incomplete',
    },
  ])
})

test('login import rejects oversized credential files before reading their contents', async () => {
  let reads = 0
  const service = new ProviderDiscoveryService({
    homeDir: '/test-home',
    env: {},
    statImpl: async (path) => ({
      isFile: () => path.endsWith('auth.json'),
      size: 1024 * 1024 + 1,
      mtimeMs: 1,
    }),
    readFileImpl: async (path) => {
      if (!path.endsWith('auth.json')) {
        throw Object.assign(new Error('missing configuration'), { code: 'ENOENT' })
      }
      reads += 1
      throw new Error('oversized credential should not be read')
    },
  })

  const discovered = await service.discover()
  assert.equal(
    discovered.providers.some((provider) => provider.source === 'codex-auth'),
    false,
  )
  assert.equal(
    discovered.errors.some((error) => error.code === 'file_too_large'),
    true,
  )
  assert.equal(reads, 0)
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

test('provider discovery reports invalid configuration files without exposing login files or ambient auth', async (t) => {
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
