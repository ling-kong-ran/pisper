import assert from 'node:assert/strict'
import test from 'node:test'
import { ProviderModelDiscoveryService } from '../services/provider-model-discovery-service.mjs'

function jsonResponse(payload, { status = 200, headers = {} } = {}) {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { 'content-type': 'application/json', ...headers },
  })
}

test('discovers and normalizes OpenAI-compatible model IDs with bearer authentication', async () => {
  const calls = []
  const service = new ProviderModelDiscoveryService({
    fetchImpl: async (url, options) => {
      calls.push({ url: String(url), options })
      return jsonResponse({
        data: [{ id: 'gpt-5.4', name: 'GPT 5.4' }, { id: 'gpt-image-1' }, { id: 'gpt-5.4' }],
      })
    },
  })
  const result = await service.discover({
    api: 'openai-responses',
    baseUrl: 'https://api.example.test/v1/',
    apiKey: 'secret-value',
    organization: 'org-1',
  })
  assert.deepEqual(
    result.models.map((item) => item.id),
    ['gpt-5.4', 'gpt-image-1'],
  )
  // 发现的模型不再按 ID 推断用途，一律默认 chat，由用户添加时显式选择类型
  assert.equal(result.models.find((item) => item.id === 'gpt-image-1').kind, 'chat')
  assert.equal(calls[0].url, 'https://api.example.test/v1/models')
  assert.equal(calls[0].options.headers.Authorization, 'Bearer secret-value')
  assert.equal(calls[0].options.headers['OpenAI-Organization'], 'org-1')
})

test('discovers Anthropic models with the official headers', async () => {
  let request
  const service = new ProviderModelDiscoveryService({
    fetchImpl: async (url, options) => {
      request = { url: String(url), options }
      return jsonResponse({
        data: [{ id: 'claude-sonnet-4-5', display_name: 'Claude Sonnet 4.5' }],
      })
    },
  })
  const result = await service.discover({
    api: 'anthropic-messages',
    baseUrl: 'https://api.anthropic.com/v1',
    apiKey: 'Bearer anthropic-secret',
  })
  assert.equal(result.models[0].name, 'Claude Sonnet 4.5')
  assert.equal(request.options.headers['x-api-key'], 'anthropic-secret')
  assert.equal(request.options.headers['anthropic-version'], '2023-06-01')
})

test('discovers Gemini models and removes the models prefix', async () => {
  let request
  const service = new ProviderModelDiscoveryService({
    fetchImpl: async (url, options) => {
      request = { url: String(url), options }
      return jsonResponse({
        models: [
          {
            name: 'models/gemini-2.5-pro',
            displayName: 'Gemini 2.5 Pro',
            supportedGenerationMethods: ['generateContent'],
          },
        ],
      })
    },
  })
  const result = await service.discover({
    api: 'google-generative-ai',
    baseUrl: 'https://generativelanguage.googleapis.com/v1beta',
    apiKey: 'google-secret',
  })
  assert.equal(result.models[0].id, 'gemini-2.5-pro')
  assert.deepEqual(result.models[0].supportedGenerationMethods, ['generateContent'])
  assert.equal(request.options.headers['x-goog-api-key'], 'google-secret')
  assert.equal(request.url.includes('google-secret'), false)
})

test('loads every paginated model page before publishing a catalog', async () => {
  const urls = []
  const service = new ProviderModelDiscoveryService({
    fetchImpl: async (url) => {
      urls.push(String(url))
      if (urls.length === 1)
        return jsonResponse({
          data: [{ id: 'claude-first' }],
          has_more: true,
          last_id: 'claude-first',
        })
      return jsonResponse({
        data: [{ id: 'claude-second' }],
        has_more: false,
        last_id: 'claude-second',
      })
    },
  })
  const result = await service.discover({
    api: 'anthropic-messages',
    baseUrl: 'https://relay.example.test/v1',
    apiKey: 'secret',
  })
  assert.deepEqual(
    result.models.map((model) => model.id),
    ['claude-first', 'claude-second'],
  )
  assert.equal(new URL(urls[1]).searchParams.get('after_id'), 'claude-first')
})

test('normalizes provider errors without exposing credentials and rejects unsafe URLs', async () => {
  const secret = 'sk-private-model-discovery-value'
  const service = new ProviderModelDiscoveryService({
    fetchImpl: async () =>
      jsonResponse({ error: { message: `invalid api_key=${secret}` } }, { status: 401 }),
  })
  await assert.rejects(
    () =>
      service.discover({
        api: 'openai-completions',
        baseUrl: 'https://api.example.test/v1',
        apiKey: secret,
      }),
    (error) => error.message.includes(secret) === false && /401/.test(error.message),
  )
  await assert.rejects(
    () =>
      service.discover({
        api: 'openai-completions',
        baseUrl: 'file:///tmp/models',
        apiKey: secret,
      }),
    /HTTP/,
  )
})

test('rejects oversized model responses', async () => {
  const service = new ProviderModelDiscoveryService({
    maxResponseBytes: 20,
    fetchImpl: async () => jsonResponse({ data: [{ id: 'model-that-is-too-large' }] }),
  })
  await assert.rejects(
    () => service.discover({ api: 'openai-responses', baseUrl: 'https://api.example.test/v1' }),
    /过大/,
  )
})

test('anthropic-messages discovery targets /v1/models when the base URL lacks the version prefix', async () => {
  const urls = []
  const service = new ProviderModelDiscoveryService({
    fetchImpl: async (url) => {
      urls.push(String(url))
      return jsonResponse({ data: [{ id: 'k3' }] })
    },
  })
  // kimi-coding 等兼容端点的 baseUrl 不含 /v1：模型列表需补 /v1 前缀（否则 404）
  await service.discover({
    api: 'anthropic-messages',
    baseUrl: 'https://api.kimi.com/coding/',
    apiKey: 'test-key',
  })
  assert.equal(urls[0], 'https://api.kimi.com/coding/v1/models')
  // 官方端点已带 /v1：只有一个候选，绝不叠加出 /v1/v1
  await service.discover({
    api: 'anthropic-messages',
    baseUrl: 'https://api.anthropic.com/v1',
    apiKey: 'test-key',
  })
  assert.equal(urls[1], 'https://api.anthropic.com/v1/models')
  // 非 anthropic 协议保持原行为
  await service.discover({
    api: 'openai-responses',
    baseUrl: 'https://api.openai.com/v1',
    apiKey: 'test-key',
  })
  assert.equal(urls[2], 'https://api.openai.com/v1/models')
})

test('discovery falls back to the /v1-prefixed candidate when the direct one 404s', async () => {
  const urls = []
  const service = new ProviderModelDiscoveryService({
    fetchImpl: async (url) => {
      const value = String(url)
      urls.push(value)
      // kimi 的 coding 端点即使用 openai 协议也挂在 /coding/v1 下
      if (value === 'https://api.kimi.com/coding/models') return jsonResponse({}, { status: 404 })
      return jsonResponse({ data: [{ id: 'k3' }] })
    },
  })
  const result = await service.discover({
    api: 'openai-completions',
    baseUrl: 'https://api.kimi.com/coding/',
    apiKey: 'test-key',
  })
  assert.deepEqual(urls, [
    'https://api.kimi.com/coding/models',
    'https://api.kimi.com/coding/v1/models',
  ])
  assert.equal(result.models[0].id, 'k3')
})

test('discovery never duplicates the /v1 prefix when the base URL already ends with it', async () => {
  const urls = []
  const service = new ProviderModelDiscoveryService({
    fetchImpl: async (url) => {
      const value = String(url)
      urls.push(value)
      // 唯一候选 404 时直接报错，不再尝试拼接 /v1/v1
      return jsonResponse({}, { status: 404 })
    },
  })
  await assert.rejects(
    () =>
      service.discover({
        api: 'openai-completions',
        baseUrl: 'https://api.example.test/v1',
        apiKey: 'test-key',
      }),
    /404/,
  )
  assert.deepEqual(urls, ['https://api.example.test/v1/models'])
})
