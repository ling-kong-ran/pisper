import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

test('model configuration exposes built-in Kimi and GLM providers', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-catalog-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  const config = await runtime.getConfig()
  const openai = config.providers.find((provider) => provider.id === 'openai')
  const kimi = config.providers.find((provider) => provider.id === 'kimi-coding')
  const glm = config.providers.find((provider) => provider.id === 'zai-coding-cn')

  assert.equal(openai.baseUrl, 'https://api.openai.com/v1')
  assert.equal(kimi.name, 'Kimi Code')
  assert.equal(kimi.api, 'anthropic-messages')
  assert.equal(kimi.baseUrl, 'https://api.kimi.com/coding/')
  assert.ok(kimi.models.some((model) => model.id === 'k3'))
  assert.equal(glm.name, 'GLM')
  assert.equal(glm.api, 'openai-completions')
  assert.equal(glm.baseUrl, 'https://open.bigmodel.cn/api/paas/v4')
  assert.ok(glm.models.some((model) => model.id === 'glm-5.2'))

  await runtime.saveConfig({
    provider: 'zai-coding-cn',
    model: 'glm-5.2',
    apiKey: 'test-key',
    baseUrl: glm.baseUrl,
    thinkingLevel: 'medium',
    toolMode: 'read-only',
  })
  assert.equal(
    runtime.modelRuntime.getModel('zai-coding-cn', 'glm-5.2').baseUrl,
    'https://open.bigmodel.cn/api/paas/v4',
  )
  assert.equal(
    (await runtime.getConfig()).providers.find((provider) => provider.id === 'zai-coding-cn')
      .configured,
    true,
  )
})

test('provider API keys update without changing the active model configuration', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-api-key-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  const before = await runtime.getConfig()
  const apiKey = ['terminal', 'secret', 'value'].join('-')

  const saved = await runtime.setProviderApiKey('kimi-coding', { apiKey })
  const after = runtime.settingsManager.getGlobalSettings()

  assert.equal(saved.apiKeyUpdated, true)
  assert.equal(saved.updatedProviderId, 'kimi-coding')
  assert.equal(after.defaultProvider, before.provider)
  assert.equal(after.defaultModel, before.model)
  assert.equal(saved.providers.find((provider) => provider.id === 'kimi-coding').configured, true)
  assert.equal(JSON.stringify(saved).includes(apiKey), false)
  const credentials = JSON.parse(await readFile(join(directory, 'auth.json'), 'utf8'))
  assert.equal(credentials['kimi-coding'].key, apiKey)
  await assert.rejects(() => runtime.setProviderApiKey('missing', { apiKey }), /Provider 不存在/)
  await assert.rejects(() => runtime.setProviderApiKey('openai', { apiKey: '   ' }), /不能为空/)
})

test('provider connections update protocol, effective Base URL, and optional API Key only', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-connection-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  const before = await runtime.getConfig()
  const defaultBaseUrl = 'https://api.openai.com/v1'
  const apiKey = ['terminal', 'connection', 'secret'].join('-')

  const saved = await runtime.setProviderConnection('openai', {
    api: 'openai-completions',
    baseUrl: defaultBaseUrl,
    apiKey,
  })
  assert.equal(saved.connectionUpdated, true)
  assert.equal(saved.apiKeyUpdated, true)
  assert.equal(saved.updatedProviderId, 'openai')
  assert.equal(
    saved.providers.find((provider) => provider.id === 'openai').api,
    'openai-completions',
  )
  assert.equal(saved.providers.find((provider) => provider.id === 'openai').baseUrl, defaultBaseUrl)
  assert.equal(JSON.stringify(saved).includes(apiKey), false)
  assert.equal(saved.provider, before.provider)
  assert.equal(saved.model, before.model)

  const defaultOverlay = JSON.parse(await readFile(join(directory, 'models.json'), 'utf8'))
  assert.equal(defaultOverlay.providers.openai.api, 'openai-completions')
  assert.equal(defaultOverlay.providers.openai.baseUrl, undefined)
  const credentials = JSON.parse(await readFile(join(directory, 'auth.json'), 'utf8'))
  assert.equal(credentials.openai.key, apiKey)

  const customBaseUrl = 'https://relay.example.test/v1'
  const updated = await runtime.setProviderConnection('openai', {
    api: 'anthropic-messages',
    baseUrl: customBaseUrl,
  })
  assert.equal(updated.apiKeyUpdated, false)
  assert.equal(
    updated.providers.find((provider) => provider.id === 'openai').baseUrl,
    customBaseUrl,
  )
  const customOverlay = JSON.parse(await readFile(join(directory, 'models.json'), 'utf8'))
  assert.equal(customOverlay.providers.openai.api, 'anthropic-messages')
  assert.equal(customOverlay.providers.openai.baseUrl, customBaseUrl)
  assert.equal(JSON.parse(await readFile(join(directory, 'auth.json'), 'utf8')).openai.key, apiKey)

  await assert.rejects(
    () =>
      runtime.setProviderConnection('openai', {
        api: 'unsupported',
        baseUrl: defaultBaseUrl,
      }),
    /不受支持/,
  )
  await assert.rejects(
    () =>
      runtime.setProviderConnection('openai', {
        api: 'openai-responses',
        baseUrl: 'file:///tmp/provider',
      }),
    /HTTP 或 HTTPS/,
  )
  await assert.rejects(
    () =>
      runtime.setProviderConnection('missing', {
        api: 'openai-responses',
        baseUrl: defaultBaseUrl,
      }),
    /Provider 不存在/,
  )
})

test('saving an unauthenticated Provider fails before changing configuration', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-save-incomplete-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  const created = await runtime.createProvider({
    id: 'incomplete-relay',
    name: 'Incomplete Relay',
    api: 'openai-responses',
    baseUrl: 'https://incomplete-relay.example.test/v1',
    model: 'incomplete-relay-model',
    enabled: false,
  })
  const before = await runtime.getConfig()
  const incomplete = created.providers.find((provider) => provider.id === 'incomplete-relay')
  assert.equal(incomplete.configured, false)
  assert.equal(incomplete.enabled, false)

  await assert.rejects(
    () =>
      runtime.saveConfig({
        provider: 'incomplete-relay',
        providerType: 'chat',
        model: 'incomplete-relay-model',
        api: 'openai-responses',
        baseUrl: 'https://incomplete-relay.example.test/v1',
        thinkingLevel: 'xhigh',
        toolMode: 'read-only',
        setAsDefault: false,
        enabled: true,
      }),
    /填写 API Key 或加载 Provider 认证/,
  )

  const after = await runtime.getConfig()
  const unchanged = after.providers.find((provider) => provider.id === 'incomplete-relay')
  assert.equal(unchanged.configured, false)
  assert.equal(unchanged.enabled, false)
  assert.equal(after.toolMode, before.toolMode)
  assert.equal(after.thinkingLevel, before.thinkingLevel)
  assert.equal(after.defaultProvider, before.defaultProvider)
  assert.equal(after.defaultModel, before.defaultModel)
})

test('saving Provider settings explicitly enables a disabled Provider', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-save-enable-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  const before = await runtime.getConfig()
  const created = await runtime.createProvider({
    id: 'disabled-relay',
    name: 'Disabled Relay',
    api: 'openai-responses',
    baseUrl: 'https://disabled-relay.example.test/v1',
    apiKey: 'disabled-relay-key',
    model: 'disabled-relay-model',
    enabled: false,
  })
  assert.equal(
    created.providers.find((provider) => provider.id === 'disabled-relay').enabled,
    false,
  )
  assert.equal(created.defaultProvider, before.defaultProvider)
  assert.equal(created.defaultModel, before.defaultModel)

  const saved = await runtime.saveConfig({
    provider: 'disabled-relay',
    providerType: 'chat',
    model: 'disabled-relay-model',
    api: 'openai-responses',
    baseUrl: 'https://disabled-relay.example.test/v1',
    thinkingLevel: 'medium',
    toolMode: 'read-only',
    setAsDefault: false,
    enabled: true,
  })

  assert.equal(saved.providers.find((provider) => provider.id === 'disabled-relay').enabled, true)
  assert.equal(saved.defaultProvider, before.defaultProvider)
  assert.equal(saved.defaultModel, before.defaultModel)
})

test('visual-only providers save connection settings without replacing the default chat model', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-visual-provider-config-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  await runtime.refreshProviderModels()
  const before = runtime.settingsManager.getGlobalSettings()
  await runtime.createProvider({
    id: 'visual-relay',
    name: 'Visual Relay',
    api: 'openai-responses',
    baseUrl: 'https://visual.example.test/v1',
    apiKey: 'visual-key',
    model: 'gpt-image-1',
    modelKind: 'image',
  })
  const nextKey = ['next', 'visual', 'credential'].join('-')
  const saved = await runtime.saveConfig({
    provider: 'visual-relay',
    model: '',
    apiKey: nextKey,
    baseUrl: 'https://visual.example.test/v1',
    thinkingLevel: 'medium',
    toolMode: 'read-only',
  })
  assert.equal(saved.apiKeyUpdated, true)
  const credentials = JSON.parse(await readFile(join(directory, 'auth.json'), 'utf8'))
  assert.equal(credentials['visual-relay'].key, nextKey)
  assert.equal(
    (await runtime.visualGeneration.models.select('image', 'visual-relay/gpt-image-1')).apiKey,
    nextKey,
  )
  const retained = await runtime.saveConfig({
    provider: 'visual-relay',
    model: '',
    baseUrl: 'https://visual.example.test/v1',
    thinkingLevel: 'medium',
    toolMode: 'read-only',
  })
  assert.equal(retained.apiKeyUpdated, false)
  assert.equal(
    (await runtime.visualGeneration.models.select('image', 'visual-relay/gpt-image-1')).apiKey,
    nextKey,
  )
  const after = runtime.settingsManager.getGlobalSettings()
  assert.equal(after.defaultProvider, before.defaultProvider)
  assert.equal(after.defaultModel, before.defaultModel)
  const visual = saved.providers.find((provider) => provider.id === 'visual-relay')
  assert.equal(visual.type, 'visual')
  assert.equal(
    visual.models.some((model) => model.kind === 'chat'),
    false,
  )
  assert.ok(visual.models.some((model) => model.kind === 'image'))
})

test('each chat provider keeps its saved default model independently', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-default-models-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  await runtime.createProvider({
    id: 'relay-one',
    name: 'Relay One',
    api: 'openai-responses',
    baseUrl: 'https://relay-one.example.test/v1',
    apiKey: 'relay-one-key',
    model: 'relay-one-first',
  })
  await runtime.addProviderModel('relay-one', {
    id: 'relay-one-second',
    name: 'Relay One Second',
    kind: 'chat',
  })
  await runtime.createProvider({
    id: 'relay-two',
    name: 'Relay Two',
    api: 'openai-responses',
    baseUrl: 'https://relay-two.example.test/v1',
    apiKey: 'relay-two-key',
    model: 'relay-two-first',
  })

  const originalDefault = runtime.settingsManager.getGlobalSettings()
  const savedWithoutDefault = await runtime.saveConfig({
    provider: 'relay-one',
    providerType: 'chat',
    model: 'relay-one-second',
    baseUrl: 'https://relay-one.example.test/v1',
    thinkingLevel: 'medium',
    toolMode: 'read-only',
    setAsDefault: false,
  })
  const retainedDefault = runtime.settingsManager.getGlobalSettings()
  assert.equal(savedWithoutDefault.defaultUpdated, false)
  assert.equal(savedWithoutDefault.defaultProvider, originalDefault.defaultProvider)
  assert.equal(savedWithoutDefault.defaultModel, originalDefault.defaultModel)
  assert.equal(retainedDefault.defaultProvider, originalDefault.defaultProvider)
  assert.equal(retainedDefault.defaultModel, originalDefault.defaultModel)

  const savedAsDefault = await runtime.saveConfig({
    provider: 'relay-two',
    providerType: 'chat',
    model: 'relay-two-first',
    baseUrl: 'https://relay-two.example.test/v1',
    thinkingLevel: 'medium',
    toolMode: 'read-only',
    setAsDefault: true,
  })
  assert.equal(savedAsDefault.defaultUpdated, true)
  assert.equal(savedAsDefault.defaultProvider, 'relay-two')
  assert.equal(savedAsDefault.defaultModel, 'relay-two-first')

  const config = await runtime.getConfig()
  assert.equal(config.provider, 'relay-two')
  assert.equal(config.model, 'relay-two-first')
  assert.equal(
    config.providers.find((provider) => provider.id === 'relay-one').defaultModel,
    'relay-one-second',
  )
  assert.equal(
    config.providers.find((provider) => provider.id === 'relay-two').defaultModel,
    'relay-two-first',
  )
})
