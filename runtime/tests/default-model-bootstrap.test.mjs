import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

// 首次安装兜底：导入/新建首个 Provider 时必须写入全局默认模型，
// 否则新会话会回退到内置兜底模型（可能与账号权限不符，表现为 404）。
async function readSettings(directory) {
  return JSON.parse(await readFile(join(directory, 'settings.json'), 'utf8'))
}

test('creating the first provider bootstraps the global default model', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-default-bootstrap-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()

  const created = await runtime.createProvider({
    name: 'Relay',
    id: 'relay-test',
    api: 'openai-responses',
    baseUrl: 'https://relay.example.test/v1',
    model: 'relay-coder',
    apiKey: 'test-key',
  })
  assert.equal(created.createdProviderId, 'relay-test')

  const settings = await readSettings(directory)
  assert.equal(settings.defaultProvider, 'relay-test')
  assert.equal(settings.defaultModel, 'relay-coder')
  assert.equal(created.defaultProvider, 'relay-test')
  assert.equal(created.defaultModel, 'relay-coder')

  // 已有默认时新建连接不覆盖用户选择
  const second = await runtime.createProvider({
    name: 'Relay Two',
    id: 'relay-two',
    api: 'openai-responses',
    baseUrl: 'https://relay-two.example.test/v1',
    model: 'relay-coder-2',
    apiKey: 'test-key-2',
  })
  const settingsAfter = await readSettings(directory)
  assert.equal(settingsAfter.defaultProvider, 'relay-test')
  assert.equal(settingsAfter.defaultModel, 'relay-coder')
  assert.equal(second.defaultProvider, 'relay-test')
})

test('importing a discovered provider bootstraps the global default model when unset', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-default-import-'))
  const providerDiscovery = {
    async discover() {
      return { providers: [], errors: [] }
    },
    async loadConfiguration() {
      return {
        kind: 'configuration',
        providerId: 'kimi-coding',
        source: 'claude-config',
        fingerprint: 'fingerprint-bootstrap',
        providerConfig: {
          name: 'Kimi Code',
          api: 'anthropic-messages',
          baseUrl: 'https://api.kimi.com/coding/',
        },
        // selectedModel 为空时按 modelRank 兜底：kimi-coding 应选中 k3 而非注册顺序第一的 k2p7。
        selectedModel: '',
        credential: { type: 'api_key', key: 'imported-key' },
      }
    },
  }
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory, providerDiscovery })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()

  const imported = await runtime.importDiscoveredProvider('any-discovery-id')
  assert.equal(imported.providerId, 'kimi-coding')
  const settings = await readSettings(directory)
  assert.equal(settings.defaultProvider, 'kimi-coding')
  assert.equal(settings.defaultModel, 'k3')
  assert.equal(imported.config.defaultProvider, 'kimi-coding')
  assert.equal(imported.config.defaultModel, 'k3')
})

test('anthropic-messages base URLs drop the trailing /v1 before persisting (no /v1/v1)', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-baseurl-normalize-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()

  // anthropic-messages：尾部 /v1 会被剥掉（SDK 会自行拼 /v1/messages）
  await runtime.createProvider({
    name: 'Kimi Relay',
    id: 'kimi-relay',
    api: 'anthropic-messages',
    baseUrl: 'https://api.kimi.com/coding/v1',
    model: 'k3',
    apiKey: 'test-key',
  })
  let modelsJson = JSON.parse(await readFile(join(directory, 'models.json'), 'utf8'))
  assert.equal(modelsJson.providers['kimi-relay'].baseUrl, 'https://api.kimi.com/coding')

  // openai 协议保留 /v1（OpenAI SDK 直接拼 /chat/completions）
  await runtime.createProvider({
    name: 'OpenAI Relay',
    id: 'openai-relay',
    api: 'openai-responses',
    baseUrl: 'https://relay.example.test/v1',
    model: 'relay-coder',
    apiKey: 'test-key',
  })
  modelsJson = JSON.parse(await readFile(join(directory, 'models.json'), 'utf8'))
  assert.equal(modelsJson.providers['openai-relay'].baseUrl, 'https://relay.example.test/v1')

  // saveConfig 路径同样归一化
  await runtime.saveConfig({
    provider: 'kimi-relay',
    providerType: 'chat',
    api: 'anthropic-messages',
    baseUrl: 'https://api.kimi.com/coding/v1/',
    model: 'k3',
    thinkingLevel: 'medium',
    toolMode: 'full',
    setAsDefault: false,
    enabled: true,
  })
  modelsJson = JSON.parse(await readFile(join(directory, 'models.json'), 'utf8'))
  assert.equal(modelsJson.providers['kimi-relay'].baseUrl, 'https://api.kimi.com/coding')
})
