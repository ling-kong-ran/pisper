import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { ProviderDiscoveryService } from '../services/provider-discovery.mjs'
import { modelCapabilities, updateModelOptions } from '../services/provider-model-options.mjs'

async function fixture(t, extra = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-model-options-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory, ...extra })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  return { runtime, directory }
}
async function create(runtime) {
  await runtime.createProvider({
    id: 'capability-relay',
    name: 'Capabilities',
    api: 'openai-completions',
    baseUrl: 'http://127.0.0.1:9/v1',
    apiKey: 'capability-fixture-key',
    model: 'fixture-model',
    reasoning: true,
  })
}

test('model options validate before mutation and preserve legacy kind', () => {
  assert.deepEqual(modelCapabilities({ kind: 'image' }), ['image'])
  const existing = { id: 'test', kind: 'chat', reasoning: true }
  const updated = updateModelOptions(existing, {
    capabilities: ['image', 'chat'],
    thinkingLevels: ['low', 'high'],
  })
  assert.deepEqual(updated.capabilities, ['chat', 'image'])
  assert.equal(updated.kind, 'chat')
  assert.equal(updated.thinkingLevelMap.medium, null)
  assert.equal(updated.thinkingLevelMap.high, 'high')
  for (const invalid of [
    { capabilities: [] },
    { capabilities: ['audio'] },
    { thinkingLevels: ['ultra'] },
    { input: ['image'] },
    { reasoning: 'yes' },
    { contextWindow: 0 },
    { maxTokens: 1.2 },
    { name: 23 },
  ])
    assert.throws(() => updateModelOptions(existing, invalid))
  assert.deepEqual(existing, { id: 'test', kind: 'chat', reasoning: true })
})

test('image-only model reuses connection credentials and cannot be selected for chat', async (t) => {
  const { runtime, directory } = await fixture(t)
  await create(runtime)
  await runtime.addProviderModels('capability-relay', [{ id: 'fallback-chat', kind: 'chat' }])
  const before = await readFile(join(directory, 'auth.json'), 'utf8')
  const config = await runtime.setProviderModelOptions('capability-relay', {
    modelId: 'fixture-model',
    capabilities: ['image'],
    name: 'Image fixture',
  })
  const model = config.providers
    .find((p) => p.id === 'capability-relay')
    .models.find((m) => m.id === 'fixture-model')
  assert.deepEqual(model.capabilities, ['image'])
  assert.equal(model.kind, 'image')
  assert.equal(config.model, 'fallback-chat')
  assert.equal(
    runtime.modelRuntime.getModel('capability-relay', 'fixture-model').pisperKind,
    'image',
  )
  await assert.rejects(
    () => runtime.providerPreferences.resolveSessionModel('capability-relay', 'fixture-model'),
    /对话能力/,
  )
  const visual = await runtime.getVisualModelStatus()
  assert.equal(visual.image.providerId, 'capability-relay')
  assert.equal(visual.image.id, 'fixture-model')
  assert.equal(await readFile(join(directory, 'auth.json'), 'utf8'), before)
  assert.ok(!JSON.stringify(config).includes('capability-fixture-key'))
  await runtime.setProviderModelOptions('capability-relay', {
    modelId: 'fixture-model',
    capabilities: ['chat', 'image'],
    reasoning: true,
    input: ['text', 'image'],
    thinkingLevels: ['low', 'high'],
    contextWindow: 65536,
    maxTokens: 4096,
  })
  await runtime.providerPreferences.reload()
  const updated = (await runtime.getConfig()).providers
    .find((p) => p.id === 'capability-relay')
    .models.find((m) => m.id === 'fixture-model')
  assert.deepEqual(updated.capabilities, ['chat', 'image'])
  assert.deepEqual(updated.thinkingLevels, ['off', 'low', 'high'])
  assert.equal(updated.contextWindow, 65536)
  assert.equal(updated.maxTokens, 4096)
  assert.equal((await runtime.getVisualModelStatus()).image.id, 'fixture-model')
})

test('model overrides survive discovery, independent saves and rejected input', async (t) => {
  const { runtime, directory } = await fixture(t, {
    providerModelDiscovery: {
      async discover() {
        return {
          models: [
            { id: 'fixture-model', name: 'Remote name', kind: 'chat' },
            { id: 'second-model', kind: 'chat' },
          ],
        }
      },
    },
  })
  await create(runtime)
  await runtime.discoverProviderModels('capability-relay')
  await Promise.all([
    runtime.setProviderModelOptions('capability-relay', {
      modelId: 'fixture-model',
      name: 'Personal name',
      capabilities: ['image'],
      reasoning: false,
    }),
    runtime.setProviderModelOptions('capability-relay', {
      modelId: 'second-model',
      capabilities: ['chat'],
      thinkingLevels: ['high'],
      reasoning: true,
    }),
  ])
  await runtime.discoverProviderModels('capability-relay')
  const provider = (await runtime.getConfig()).providers.find((p) => p.id === 'capability-relay')
  assert.equal(provider.models.find((m) => m.id === 'fixture-model').kind, 'image')
  assert.equal(provider.models.find((m) => m.id === 'fixture-model').name, 'Personal name')
  assert.deepEqual(provider.models.find((m) => m.id === 'second-model').thinkingLevels, [
    'off',
    'high',
  ])
  const before = await readFile(join(directory, 'models.json'), 'utf8')
  await assert.rejects(
    () =>
      runtime.setProviderModelOptions('capability-relay', {
        modelId: 'fixture-model',
        capabilities: [],
      }),
    /有效的模型能力/,
  )
  assert.equal(await readFile(join(directory, 'models.json'), 'utf8'), before)
})

test('new model accepts image capability on an existing chat connection', async (t) => {
  const { runtime } = await fixture(t)
  await create(runtime)
  const config = await runtime.addProviderModel('capability-relay', {
    id: 'only-image',
    capabilities: ['image'],
    kind: 'image',
    name: 'Only image',
  })
  const provider = config.providers.find((p) => p.id === 'capability-relay')
  assert.equal(provider.type, 'chat')
  assert.equal(provider.models.find((m) => m.id === 'only-image').kind, 'image')
  assert.equal((await runtime.getVisualModelStatus()).image.id, 'only-image')
})

test('manual image and mixed capability models survive catalog refresh and runtime reload', async (t) => {
  const { runtime } = await fixture(t, {
    providerModelDiscovery: {
      async discover() {
        return { models: [{ id: 'fixture-model', kind: 'chat' }] }
      },
    },
  })
  await create(runtime)
  await runtime.discoverProviderModels('capability-relay')
  await runtime.addProviderModel('capability-relay', {
    id: 'manual-image',
    capabilities: ['image'],
    name: 'Manual image',
  })
  for (const capabilities of [['image'], ['chat', 'image']]) {
    await runtime.setProviderModelOptions('capability-relay', {
      modelId: 'manual-image',
      capabilities,
      input: ['text', 'image'],
      contextWindow: 32000,
      maxTokens: 4096,
    })
    await runtime.refreshProviderModels()
    await runtime.providerPreferences.reload()
    const model = (await runtime.getConfig()).providers
      .find((p) => p.id === 'capability-relay')
      .models.find((m) => m.id === 'manual-image')
    assert.ok(model, 'manual model remains visible even if the remote catalog omits it')
    assert.deepEqual(model.capabilities, capabilities)
    assert.equal(model.contextWindow, 32000)
    assert.equal(model.maxTokens, 4096)
  }
})

test('automatic local import handles Codex bearer and Claude, is idempotent and never exposes credentials', async (t) => {
  const home = await mkdtemp(join(tmpdir(), 'pisper-local-provider-home-'))
  t.after(() => rm(home, { recursive: true, force: true }))
  await mkdir(join(home, '.codex'))
  await mkdir(join(home, '.claude'))
  await writeFile(
    join(home, '.codex', 'config.toml'),
    'model_provider="custom"\nmodel="local-codex"\n[model_providers.custom]\nname="Local Codex"\nbase_url="http://127.0.0.1:9/v1"\nwire_api="responses"\nexperimental_bearer_token="codex-bearer-test-secret"',
  )
  await writeFile(
    join(home, '.claude', 'settings.json'),
    JSON.stringify({
      model: 'local-claude',
      env: { ANTHROPIC_BASE_URL: 'http://127.0.0.1:9', ANTHROPIC_AUTH_TOKEN: 'claude-test-secret' },
    }),
  )
  const discovery = new ProviderDiscoveryService({ homeDir: home, env: {} })
  const publicResult = await discovery.discover()
  assert.equal(
    publicResult.providers.find((p) => p.source === 'codex-config').credentialPresent,
    true,
  )
  assert.ok(!JSON.stringify(publicResult).includes('codex-bearer-test-secret'))
  const { runtime, directory } = await fixture(t, { providerDiscovery: discovery })
  const [first, duplicate] = await Promise.all([
    runtime.importLocalProviders(),
    runtime.importLocalProviders(),
  ])
  assert.equal(first.imported.length, 2)
  assert.deepEqual(first, duplicate)
  assert.ok(!JSON.stringify(first).includes('codex-bearer-test-secret'))
  assert.ok(!JSON.stringify(first).includes('claude-test-secret'))
  const before = await readFile(join(directory, 'auth.json'), 'utf8')
  assert.equal((await runtime.importLocalProviders()).imported.length, 0)
  assert.equal(await readFile(join(directory, 'auth.json'), 'utf8'), before)
  assert.equal(JSON.parse(before)['codex-custom'].key, 'codex-bearer-test-secret')
  assert.equal(first.config.providers.find((p) => p.id === 'codex-custom').configured, true)
})

test('automatic import does not overwrite a preconfigured Provider or guess missing credentials', async (t) => {
  const credential = 'external-test-secret'
  const discovery = {
    async discover() {
      return {
        providers: [
          {
            id: 'conflict',
            providerId: 'capability-relay',
            kind: 'configuration',
            source: 'codex-config',
            importable: true,
            credentialPresent: true,
            fingerprint: 'different',
          },
          {
            id: 'no-auth',
            providerId: 'missing',
            source: 'codex-config',
            importable: true,
            credentialPresent: false,
          },
        ],
        errors: [],
      }
    },
    async loadConfiguration() {
      throw Error('Must not import')
    },
  }
  const { runtime, directory } = await fixture(t, { providerDiscovery: discovery })
  await create(runtime)
  const before = await readFile(join(directory, 'auth.json'), 'utf8')
  const result = await runtime.importLocalProviders()
  assert.equal(result.imported.length, 0)
  assert.deepEqual(
    result.skipped.map((item) => item.reason),
    ['conflict', 'authentication_required'],
  )
  assert.equal(await readFile(join(directory, 'auth.json'), 'utf8'), before)
  assert.ok(!JSON.stringify(result).includes(credential))
})

// 公共接口保持增量兼容：模型元数据走独立入口，不借全局配置改变运行策略。
test('model options and local import HTTP endpoints preserve the payload and reject legacy config routing', async () => {
  const calls = []
  const handler = createApiHandler({
    async setProviderModelOptions(id, options) {
      calls.push({ id, options })
      return { providers: [{ id, models: [options] }] }
    },
    async importLocalProviders() {
      calls.push('local')
      return {
        config: { providers: [] },
        discovery: { providers: [], errors: [] },
        imported: [],
        skipped: [],
      }
    },
    async saveConfig() {
      throw new Error('must not use global config')
    },
  })
  async function request(method, path, body) {
    const res = {
      status: 0,
      body: '',
      writeHead(status) {
        this.status = status
      },
      end(body) {
        this.body = body
      },
    }
    const req = {
      method,
      async *[Symbol.asyncIterator]() {
        yield Buffer.from(JSON.stringify(body ?? {}))
      },
    }
    assert.equal(await handler(req, res, new URL('http://localhost' + path)), true)
    return { status: res.status, body: JSON.parse(res.body) }
  }
  const options = { modelId: 'image-fixture', capabilities: ['image'] }
  const saved = await request('PUT', '/api/providers/relay/models/options', options)
  assert.equal(saved.status, 200)
  assert.deepEqual(saved.body.providers[0].models[0], options)
  const imported = await request('POST', '/api/providers/import-local')
  assert.equal(imported.status, 200)
  assert.deepEqual(imported.body.imported, [])
  assert.deepEqual(calls, [{ id: 'relay', options }, 'local'])
})
