import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

test('provider model discovery uses the configured relay Base URL and stored credential', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-model-runtime-'))
  const calls = []
  const runtime = new AgentRuntimeService({
    cwd: directory,
    dataDir: directory,
    appVersion: '0.3.2-test',
    providerModelDiscovery: {
      async discover(input) {
        calls.push(input)
        return {
          count: 2,
          models: [
            { id: 'relay-chat-v2', name: 'Relay Chat V2', kind: 'chat' },
            { id: 'relay-image-v1', name: 'Relay Image V1', kind: 'image' },
          ],
        }
      },
    },
  })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  await runtime.createProvider({
    id: 'company-relay',
    name: 'Company Relay',
    api: 'openai-responses',
    baseUrl: 'https://relay.example.test/v1',
    apiKey: 'relay-private-key',
    model: 'relay-chat-v1',
  })

  const discovered = await runtime.discoverProviderModels('company-relay')
  assert.equal(calls.length, 1)
  assert.equal(calls[0].baseUrl, 'https://relay.example.test/v1')
  assert.equal(calls[0].apiKey, 'relay-private-key')
  assert.equal(calls[0].headers['User-Agent'], 'Pisper/0.3.2-test')
  assert.equal(discovered.synchronized, true)
  assert.equal(
    discovered.models.every((model) => model.added === true),
    true,
  )
  assert.deepEqual(discovered.addedModelIds, ['relay-chat-v2', 'relay-image-v1'])
  assert.deepEqual(discovered.removedModelIds, ['relay-chat-v1'])
  assert.equal(runtime.modelRuntime.getModel('company-relay', 'relay-chat-v1'), undefined)
  assert.equal(
    runtime.modelRuntime.getModel('company-relay', 'relay-chat-v2').headers['User-Agent'],
    'Pisper/0.3.2-test',
  )
  const provider = discovered.config.providers.find((item) => item.id === 'company-relay')
  assert.ok(provider.models.some((model) => model.id === 'relay-chat-v2' && model.kind === 'chat'))
  assert.ok(
    provider.models.some((model) => model.id === 'relay-image-v1' && model.kind === 'image'),
  )
})

test('custom OpenAI Responses models send template-supported xhigh reasoning', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-thinking-runtime-'))
  let requestBody
  const server = createServer(async (request, response) => {
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    requestBody = JSON.parse(Buffer.concat(chunks).toString('utf8'))
    response.writeHead(400, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ error: { message: 'request captured' } }))
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const baseUrl = `http://127.0.0.1:${server.address().port}/v1`
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        relay: {
          name: 'Relay',
          api: 'openai-responses',
          baseUrl,
          models: [
            {
              id: 'gpt-5.6-sol',
              reasoning: true,
              input: ['text'],
              contextWindow: 200_000,
              maxTokens: 128_000,
            },
          ],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({ relay: { type: 'api_key', key: 'relay-key' } }),
  )
  const runtime = new AgentRuntimeService({
    cwd: directory,
    dataDir: directory,
    providerModelDiscovery: {
      async discover() {
        return { count: 1, models: [{ id: 'gpt-5.6-sol', name: 'GPT-5.6 Sol', kind: 'chat' }] }
      },
    },
  })
  t.after(async () => {
    await runtime.dispose()
    server.closeAllConnections?.()
    await new Promise((resolve) => server.close(resolve))
    await rm(directory, { recursive: true, force: true })
  })

  await runtime.init()
  const model = runtime.modelRuntime.getModel('relay', 'gpt-5.6-sol')
  assert.deepEqual(model.thinkingLevelMap, { off: 'none', xhigh: 'xhigh', max: 'max' })
  await runtime.modelRuntime
    .completeSimple(
      model,
      {
        systemPrompt: 'Test',
        messages: [{ role: 'user', content: 'Test', timestamp: Date.now() }],
      },
      { reasoning: 'xhigh', maxTokens: 16 },
    )
    .catch(() => {})

  assert.equal(requestBody.reasoning.effort, 'xhigh')
})

test('custom OpenAI-compatible GLM models preserve their model-specific max reasoning', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-glm-thinking-runtime-'))
  let requestBody
  const server = createServer(async (request, response) => {
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    requestBody = JSON.parse(Buffer.concat(chunks).toString('utf8'))
    response.writeHead(400, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ error: { message: 'request captured' } }))
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const baseUrl = `http://127.0.0.1:${server.address().port}/v1`
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        relay: {
          name: 'Relay',
          api: 'openai-completions',
          baseUrl,
          models: [
            {
              id: 'glm-5.2',
              reasoning: true,
              compat: { supportsReasoningEffort: true },
              input: ['text'],
              contextWindow: 200_000,
              maxTokens: 128_000,
            },
          ],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({ relay: { type: 'api_key', key: 'relay-key' } }),
  )
  const runtime = new AgentRuntimeService({
    cwd: directory,
    dataDir: directory,
    providerModelDiscovery: {
      async discover() {
        return { count: 1, models: [{ id: 'glm-5.2', name: 'GLM-5.2', kind: 'chat' }] }
      },
    },
  })
  t.after(async () => {
    await runtime.dispose()
    server.closeAllConnections?.()
    await new Promise((resolve) => server.close(resolve))
    await rm(directory, { recursive: true, force: true })
  })

  await runtime.init()
  const model = runtime.modelRuntime.getModel('relay', 'glm-5.2')
  assert.deepEqual(model.thinkingLevelMap, {
    xhigh: null,
    max: 'max',
    minimal: null,
    low: 'high',
    medium: 'high',
    high: 'high',
  })
  await runtime.modelRuntime
    .completeSimple(
      model,
      {
        systemPrompt: 'Test',
        messages: [{ role: 'user', content: 'Test', timestamp: Date.now() }],
      },
      { reasoning: 'max', maxTokens: 16 },
    )
    .catch(() => {})

  assert.equal(requestBody.reasoning_effort, 'max')
})

test('explicit relay User-Agent overrides the Pisper default', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-user-agent-'))
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        relay: {
          name: 'Relay',
          api: 'openai-completions',
          baseUrl: 'https://relay.example.test/v1',
          headers: { 'user-agent': 'Mozilla/5.0' },
          models: [{ id: 'relay-chat', name: 'Relay Chat', api: 'openai-completions' }],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({ relay: { type: 'api_key', key: 'relay-key' } }),
  )
  const requests = []
  const runtime = new AgentRuntimeService({
    cwd: directory,
    dataDir: directory,
    appVersion: '0.3.2-test',
    providerModelDiscovery: {
      async discover(input) {
        requests.push(input)
        return { count: 1, models: [{ id: 'relay-chat', name: 'Relay Chat', kind: 'chat' }] }
      },
    },
  })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })

  await runtime.init()
  await runtime.refreshProviderModels()
  assert.equal(requests.at(-1).headers['user-agent'], 'Mozilla/5.0')
  assert.equal(requests.at(-1).headers['User-Agent'], undefined)
  assert.equal(
    runtime.modelRuntime.getModel('relay', 'relay-chat').headers['user-agent'],
    'Mozilla/5.0',
  )
})

test('built-in official providers use their visible default Base URL', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-model-official-url-'))
  let request
  const runtime = new AgentRuntimeService({
    cwd: directory,
    dataDir: directory,
    providerModelDiscovery: {
      async discover(input) {
        request = input
        return { count: 1, models: [{ id: 'official-chat', name: 'Official Chat', kind: 'chat' }] }
      },
    },
  })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  const result = await runtime.discoverProviderModels('openai', { apiKey: 'private-key' })
  assert.equal(request.baseUrl, 'https://api.openai.com/v1')
  assert.equal(request.headers['User-Agent'], undefined)
  assert.equal(result.synchronized, true)
  assert.equal(
    runtime.modelRuntime.getModel('openai', 'official-chat').baseUrl,
    'https://api.openai.com/v1',
  )
})

test('startup refresh runs asynchronously and atomically replaces stale provider models', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-model-startup-refresh-'))
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        relay: {
          name: 'Relay',
          api: 'openai-responses',
          baseUrl: 'https://relay.example.test/v1',
          models: [{ id: 'stale-model', name: 'Stale Model', api: 'openai-responses' }],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({ relay: { type: 'api_key', key: 'relay-key' } }),
  )
  let release
  let startedResolve
  const started = new Promise((resolve) => {
    startedResolve = resolve
  })
  const runtime = new AgentRuntimeService({
    cwd: directory,
    dataDir: directory,
    providerModelDiscovery: {
      async discover() {
        startedResolve()
        await new Promise((resolve) => {
          release = resolve
        })
        return { count: 1, models: [{ id: 'current-model', name: 'Current Model', kind: 'chat' }] }
      },
    },
  })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })

  await runtime.init()
  await started
  assert.ok(runtime.modelRuntime.getModel('relay', 'stale-model'))
  release()
  const refreshed = await runtime.refreshProviderModels()
  assert.equal(refreshed.results.find((item) => item.provider === 'relay').ok, true)
  assert.equal(runtime.modelRuntime.getModel('relay', 'stale-model'), undefined)
  assert.ok(runtime.modelRuntime.getModel('relay', 'current-model'))
})

test('provider refresh API returns the asynchronously refreshed configuration', async () => {
  const handler = createApiHandler({
    async refreshProviderModels() {
      return {
        results: [{ provider: 'relay', ok: true }],
        config: { provider: 'relay', model: 'current-model' },
      }
    },
  })
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
  const handled = await handler(
    { method: 'POST' },
    response,
    new URL('http://localhost/api/providers/models/refresh'),
  )
  assert.equal(handled, true)
  assert.equal(response.status, 200)
  assert.equal(JSON.parse(response.body).config.model, 'current-model')
})

test('dedicated visual Providers remove shared visual models from chat Provider catalogs', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-model-shadowed-visual-'))
  const runtime = new AgentRuntimeService({
    cwd: directory,
    dataDir: directory,
    providerModelDiscovery: {
      async discover() {
        return {
          count: 2,
          models: [
            { id: 'relay-chat-v2', name: 'Relay Chat V2', kind: 'chat' },
            { id: 'gpt-image-2', name: 'GPT Image 2', kind: 'image' },
          ],
        }
      },
    },
  })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  const baseUrl = 'https://shared.example.test/v1'
  await runtime.createProvider({
    id: 'shared-chat',
    name: 'Shared Chat',
    api: 'openai-responses',
    baseUrl,
    apiKey: 'chat-key',
    model: 'relay-chat-v1',
    modelKind: 'chat',
  })
  await runtime.createProvider({
    id: 'shared-visual',
    name: 'Shared Visual',
    api: 'openai-responses',
    baseUrl,
    apiKey: 'visual-key',
    model: 'gpt-image-2',
    modelKind: 'image',
  })

  const result = await runtime.discoverProviderModels('shared-chat')
  assert.deepEqual(
    result.models.map((model) => model.id),
    ['relay-chat-v2'],
  )
  assert.equal(runtime.modelRuntime.getModel('shared-chat', 'gpt-image-2'), undefined)
  const chatProvider = result.config.providers.find((provider) => provider.id === 'shared-chat')
  const visualProvider = result.config.providers.find((provider) => provider.id === 'shared-visual')
  assert.equal(
    chatProvider.models.some((model) => model.id === 'gpt-image-2'),
    false,
  )
  assert.equal(
    visualProvider.models.some((model) => model.id === 'gpt-image-2'),
    true,
  )
})

test('visual-only providers list every discovered model so users can pick the kind', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-provider-model-visual-scope-'))
  const runtime = new AgentRuntimeService({
    cwd: directory,
    dataDir: directory,
    providerModelDiscovery: {
      async discover() {
        return {
          count: 3,
          models: [
            { id: 'gpt-image-2', name: 'gpt-image-2', kind: 'image' },
            { id: 'grok-3-mini', name: 'grok-3-mini', kind: 'chat' },
            { id: 'grok-imagine-video', name: 'grok-imagine-video', kind: 'video' },
          ],
        }
      },
    },
  })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  await runtime.createProvider({
    id: 'openai-image',
    name: '小土包的生图',
    api: 'openai-responses',
    baseUrl: 'https://visual.example.test/v1',
    apiKey: 'visual-key',
    model: 'gpt-image-2',
    modelKind: 'image',
  })

  const result = await runtime.discoverProviderModels('openai-image')
  assert.equal(result.scope, 'visual')
  // 发现结果不再按 ID 推断的 kind 过滤，全量列出，由用户添加时显式选择图像/视频类型
  assert.deepEqual(
    result.models.map((model) => model.id),
    ['gpt-image-2', 'grok-3-mini', 'grok-imagine-video'],
  )
  const provider = result.config.providers.find((item) => item.id === 'openai-image')
  assert.equal(provider.type, 'visual')
  assert.equal(
    provider.models.some((model) => model.kind === 'chat'),
    false,
  )
  assert.ok(provider.models.some((model) => model.id === 'gpt-image-2'))
  assert.ok(provider.models.some((model) => model.id === 'grok-imagine-video'))
})
