import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  availableThinkingLevelsForModel,
  thinkingLevelMapFromSelection,
} from '../runtime/provider-preferences.mjs'

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

  const saved = await runtime.saveConfig({
    provider: 'zai-coding-cn',
    providerName: '自定义 GLM 连接',
    model: 'glm-5.2',
    apiKey: 'test-key',
    baseUrl: glm.baseUrl,
    thinkingLevel: 'medium',
    toolMode: 'workspace',
  })
  assert.equal(
    saved.providers.find((provider) => provider.id === 'zai-coding-cn').name,
    '自定义 GLM 连接',
  )
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

test('configured OpenAI protocol applies to built-in Kimi models', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-kimi-protocol-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()

  await runtime.saveConfig({
    provider: 'kimi-coding',
    providerType: 'chat',
    api: 'openai-responses',
    baseUrl: 'https://api.kimi.com/coding/v1',
    model: 'k3',
    apiKey: 'test-key',
    thinkingLevel: 'medium',
    toolMode: 'workspace',
  })

  const model = runtime.modelRuntime.getModel('kimi-coding', 'k3')
  assert.equal(model.api, 'openai-responses')
  assert.equal(model.baseUrl, 'https://api.kimi.com/coding/v1')
})

test('removed tool modes use the full default without a migration file', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-tool-mode-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  await writeFile(
    join(directory, 'pisper.json'),
    JSON.stringify({ toolMode: 'unknown', enabledTools: ['read'] }),
  )

  assert.equal((await runtime.getConfig()).toolMode, 'full')
  assert.equal((await runtime.exportProviderConfig()).toolMode, 'full')
})

test('paired device model config export imports credentials and defaults into the mobile runtime', async (t) => {
  const sourceDirectory = await mkdtemp(join(tmpdir(), 'pisper-provider-export-source-'))
  const targetDirectory = await mkdtemp(join(tmpdir(), 'pisper-provider-export-target-'))
  const source = new AgentRuntimeService({ cwd: sourceDirectory, dataDir: sourceDirectory })
  const target = new AgentRuntimeService({ cwd: targetDirectory, dataDir: targetDirectory })
  t.after(async () => {
    await source.dispose()
    await target.dispose()
    await rm(sourceDirectory, { recursive: true, force: true })
    await rm(targetDirectory, { recursive: true, force: true })
  })
  await source.init()
  await target.init()
  await source.saveConfig({
    provider: 'zai-coding-cn',
    model: 'glm-5.2',
    apiKey: 'desktop-provider-secret',
    baseUrl: 'https://desktop.example.test/v1',
    thinkingLevel: 'high',
    toolMode: 'workspace',
  })
  const exported = await source.exportProviderConfig()
  assert.equal(exported.version, 1)
  assert.equal(exported.credentials['zai-coding-cn'].key, 'desktop-provider-secret')
  await target.importProviderConfig(exported)
  const imported = await target.getConfig()
  assert.equal(imported.defaultProvider, 'zai-coding-cn')
  assert.equal(imported.defaultModel, 'glm-5.2')
  assert.equal(imported.providers.find((item) => item.id === 'zai-coding-cn').configured, true)
  assert.equal(
    JSON.parse(await readFile(join(targetDirectory, 'auth.json'), 'utf8'))['zai-coding-cn'].key,
    'desktop-provider-secret',
  )
})

test('OAuth credentials are never reused for custom Provider model discovery', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-oauth-boundary-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  await writeFile(
    runtime.authPath,
    JSON.stringify({ 'openai-codex': { type: 'oauth', access: 'oauth-secret' } }),
  )
  let discoveredKey = 'not-called'
  runtime.providerModelDiscovery.discover = async ({ apiKey }) => {
    discoveredKey = apiKey
    return { models: [{ id: 'relay-model', kind: 'chat' }] }
  }

  await assert.rejects(
    runtime.discoverConnectionModels({
      providerId: 'openai-codex',
      providerType: 'chat',
      api: 'openai-responses',
      baseUrl: 'https://relay.example.test/v1',
    }),
    /OAuth.*official|官方 Provider.*OAuth/i,
  )
  assert.equal(discoveredKey, 'not-called')

  const result = await runtime.discoverConnectionModels({
    providerId: 'openai-codex',
    providerType: 'chat',
    api: 'openai-responses',
    baseUrl: 'https://relay.example.test/v1',
    apiKey: 'explicit-relay-key',
  })
  assert.equal(discoveredKey, 'explicit-relay-key')
  assert.equal(result.models[0].id, 'relay-model')
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
  // anthropic-messages 协议的 Base URL 会剥掉尾部 /v1（SDK 会自行拼 /v1/messages）
  const normalizedCustomBaseUrl = 'https://relay.example.test'
  const updated = await runtime.setProviderConnection('openai', {
    api: 'anthropic-messages',
    baseUrl: customBaseUrl,
  })
  assert.equal(updated.apiKeyUpdated, false)
  assert.equal(
    updated.providers.find((provider) => provider.id === 'openai').baseUrl,
    normalizedCustomBaseUrl,
  )
  const customOverlay = JSON.parse(await readFile(join(directory, 'models.json'), 'utf8'))
  assert.equal(customOverlay.providers.openai.api, 'anthropic-messages')
  assert.equal(customOverlay.providers.openai.baseUrl, normalizedCustomBaseUrl)
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
        toolMode: 'workspace',
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
    toolMode: 'workspace',
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
  const nextBaseUrl = 'https://visual-updated.example.test/v2'
  const saved = await runtime.saveConfig({
    provider: 'visual-relay',
    providerName: 'Updated Visual Relay',
    model: 'gpt-image-1',
    modelKind: 'image',
    apiKey: nextKey,
    baseUrl: nextBaseUrl,
    thinkingLevel: 'medium',
    toolMode: 'workspace',
  })
  assert.equal(saved.apiKeyUpdated, true)
  const credentials = JSON.parse(await readFile(join(directory, 'auth.json'), 'utf8'))
  assert.equal(credentials['visual-relay'].key, nextKey)
  assert.equal(
    saved.providers.find((provider) => provider.id === 'visual-relay').baseUrl,
    nextBaseUrl,
  )
  assert.equal(
    saved.providers.find((provider) => provider.id === 'visual-relay').name,
    'Updated Visual Relay',
  )
  const updatedVisualModel = await runtime.visualGeneration.models.select(
    'image',
    'visual-relay/gpt-image-1',
  )
  assert.equal(updatedVisualModel.apiKey, nextKey)
  assert.equal(updatedVisualModel.baseUrl, nextBaseUrl)
  const retained = await runtime.saveConfig({
    provider: 'visual-relay',
    model: '',
    baseUrl: nextBaseUrl,
    thinkingLevel: 'medium',
    toolMode: 'workspace',
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
    toolMode: 'workspace',
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
    toolMode: 'workspace',
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

test('built-in providers are only visual when explicitly marked, never inferred from stray visual models', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-builtin-visual-inference-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()

  // 内置对话 Provider 的覆盖配置中遗留视觉模型（如 openai 下的 sora-2）时，
  // 不得被推断为视觉供应商而混入「视觉连接」列表。
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        openai: {
          baseUrl: 'https://relay.example.test/v1',
          models: [{ id: 'sora-2', name: 'sora-2', kind: 'video' }],
        },
      },
    }),
  )
  const inferred = await runtime.getConfig()
  assert.equal(inferred.providers.find((provider) => provider.id === 'openai').type, 'chat')

  // 显式标记为 visual 的内置 Provider 仍然按视觉连接展示。
  await writeFile(
    join(directory, 'pisper.json'),
    JSON.stringify({ providerTypes: { openai: 'visual' } }),
  )
  const explicit = await runtime.getConfig()
  assert.equal(explicit.providers.find((provider) => provider.id === 'openai').type, 'visual')
})

test('saveConfig persists explicit model thinking levels as a complete level map', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-thinking-levels-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()

  // 本地模型（如 llama.cpp 的 Qwen3.8-27B）模板只支持 low/medium/high/xhigh：
  // 用户勾选后保存，应落盘为完整映射（未勾选等级置 null），而非依赖静态默认。
  await runtime.saveConfig({
    provider: 'local-qwen',
    providerName: '本地 Qwen',
    providerType: 'chat',
    model: 'qwen3.8-27b',
    apiKey: 'local-key',
    baseUrl: 'http://127.0.0.1:8000/v1',
    thinkingLevel: 'high',
    toolMode: 'workspace',
    thinkingLevels: ['low', 'medium', 'high', 'xhigh'],
  })

  const savedModel = JSON.parse(await readFile(join(directory, 'models.json'), 'utf8')).providers[
    'local-qwen'
  ].models.find((model) => model.id === 'qwen3.8-27b')
  assert.deepEqual(savedModel.thinkingLevelMap, {
    off: 'off',
    minimal: null,
    low: 'low',
    medium: 'medium',
    high: 'high',
    xhigh: 'xhigh',
    max: null,
  })

  // 运行时模型对象应携带用户映射（覆盖默认映射）。
  const runtimeModel = runtime.modelRuntime.getModel('local-qwen', 'qwen3.8-27b')
  assert.equal(runtimeModel.thinkingLevelMap?.minimal, null)
  assert.equal(runtimeModel.thinkingLevelMap?.xhigh, 'xhigh')

  // 等级推导与 Composer 下拉一致：无 minimal/max，含 off 与 xhigh。
  assert.deepEqual(availableThinkingLevelsForModel(runtimeModel), [
    'off',
    'low',
    'medium',
    'high',
    'xhigh',
  ])

  // getConfig 回显有效等级，供模型编辑弹窗预填。
  const config = await runtime.getConfig()
  const exposed = config.providers
    .find((provider) => provider.id === 'local-qwen')
    .models.find((model) => model.id === 'qwen3.8-27b')
  assert.deepEqual(exposed.thinkingLevels, ['off', 'low', 'medium', 'high', 'xhigh'])
})

test('thinkingLevelMapFromSelection emits a complete explicit map', () => {
  // 勾选即透传，未勾选显式 null（下拉隐藏），非法等级忽略；off 恒保留。
  assert.deepEqual(thinkingLevelMapFromSelection(['low', 'xhigh']), {
    off: 'off',
    minimal: null,
    low: 'low',
    medium: null,
    high: null,
    xhigh: 'xhigh',
    max: null,
  })
  // 即使未勾选 off，也强制保留（误配后仍能关闭思考）。
  assert.deepEqual(thinkingLevelMapFromSelection(['bogus', 'low']), {
    off: 'off',
    minimal: null,
    low: 'low',
    medium: null,
    high: null,
    xhigh: null,
    max: null,
  })
})

test('availableThinkingLevelsForModel falls back to defaults without a map', () => {
  // 未声明 thinkingLevelMap 的模型：默认映射下无 xhigh/max（保留现状）。
  assert.deepEqual(availableThinkingLevelsForModel({ reasoning: true }), [
    'off',
    'minimal',
    'low',
    'medium',
    'high',
  ])
  // 无 reasoning 仅 off。
  assert.deepEqual(availableThinkingLevelsForModel({ reasoning: false }), ['off'])
})

test('saving a model without thinkingLevels keeps prior level map untouched', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-thinking-levels-keep-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  await runtime.saveConfig({
    provider: 'local-qwen',
    providerType: 'chat',
    model: 'qwen3.8-27b',
    apiKey: 'local-key',
    baseUrl: 'http://127.0.0.1:8000/v1',
    thinkingLevel: 'medium',
    toolMode: 'workspace',
    thinkingLevels: ['low', 'medium', 'high', 'xhigh'],
  })

  // 再次保存（未提供 thinkingLevels）时保留已有映射，不清空也不臆造。
  await runtime.saveConfig({
    provider: 'local-qwen',
    providerType: 'chat',
    model: 'qwen3.8-27b',
    baseUrl: 'http://127.0.0.1:8000/v1',
    thinkingLevel: 'high',
    toolMode: 'workspace',
  })
  const savedModel = JSON.parse(await readFile(join(directory, 'models.json'), 'utf8')).providers[
    'local-qwen'
  ].models.find((model) => model.id === 'qwen3.8-27b')
  assert.deepEqual(savedModel.thinkingLevelMap, {
    off: 'off',
    minimal: null,
    low: 'low',
    medium: 'medium',
    high: 'high',
    xhigh: 'xhigh',
    max: null,
  })
})

test('createProvider persists initial model thinking levels', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-thinking-levels-create-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()

  await runtime.createProvider({
    id: 'local-qwen',
    name: '本地 Qwen',
    providerType: 'chat',
    api: 'openai-responses',
    baseUrl: 'http://127.0.0.1:8000/v1',
    apiKey: 'local-key',
    model: 'qwen3.8-27b',
    modelKind: 'chat',
    enabled: true,
    thinkingLevels: ['low', 'medium', 'high', 'xhigh'],
  })

  const savedModel = JSON.parse(await readFile(join(directory, 'models.json'), 'utf8')).providers[
    'local-qwen'
  ].models.find((model) => model.id === 'qwen3.8-27b')
  assert.deepEqual(savedModel.thinkingLevelMap, {
    off: 'off',
    minimal: null,
    low: 'low',
    medium: 'medium',
    high: 'high',
    xhigh: 'xhigh',
    max: null,
  })
  const exposed = (await runtime.getConfig()).providers
    .find((provider) => provider.id === 'local-qwen')
    .models.find((model) => model.id === 'qwen3.8-27b')
  assert.deepEqual(exposed.thinkingLevels, ['off', 'low', 'medium', 'high', 'xhigh'])
})
