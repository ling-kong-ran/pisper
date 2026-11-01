import assert from 'node:assert/strict'
import { createServer } from 'node:https'
import test from 'node:test'
import selfsigned from 'selfsigned'
import {
  ProviderRequestService,
  parseProviderRequest,
} from '../services/provider-request-service.mjs'
import { ComponentFetchProxy } from '../services/custom-ui-fetch-proxy.mjs'
import { normalizeNativeManifest } from '../../shared/custom-ui-manifest.mjs'
import { lookupProviderAddresses } from '../security/provider-request-dns.mjs'

const SECRET = 'fixture-session-credential'
const result = (body = '{"remaining":75}') => ({
  status: 200,
  statusText: 'OK',
  headers: { 'content-type': 'application/json' },
  body,
  bodyEncoding: 'utf8',
  url: 'https://provider.example/usage',
  redirected: false,
})
function fixture(options = {}) {
  const calls = { auth: [], send: [] }
  const model = {
    provider: 'custom-gateway',
    api: 'openai-completions',
    baseUrl: 'https://provider.example/v1',
    ...options.model,
  }
  const transport = new ComponentFetchProxy({
    perMinute: options.perMinute ?? 30,
    concurrent: options.concurrent ?? 4,
  })
  transport.sendAuthenticated = async (hop, signal) => {
    calls.send.push({ hop, signal })
    if (options.send) return options.send(hop, signal)
    return options.result ?? result()
  }
  const service = new ProviderRequestService({
    getSession: options.getSession ?? (async (id) => ({ session: { model: { ...model, id } } })),
    getModelRuntime: () => ({
      getAuth:
        options.getAuth ??
        (async (received, opts) => {
          calls.auth.push({ model: received, signal: opts.signal })
          return { auth: { apiKey: SECRET, ...options.auth } }
        }),
    }),
    transport: options.transport ?? transport,
    timeoutMs: options.timeoutMs ?? 1000,
  })
  service.selectSession('session-a')
  return { service, calls, transport }
}
const request = (f, input = { path: '/usage' }, signal) =>
  f.service.request('widget', 'session-a', input, signal)

test('通用请求支持自定义网关、根路径、查询、方法和正文，认证不进入响应', async () => {
  const f = fixture()
  assert.deepEqual(
    await request(f, {
      path: '/usage?period=week',
      method: 'POST',
      body: '{"x":1}',
      headers: { accept: 'application/json' },
    }),
    result(),
  )
  const { hop, signal } = f.calls.send[0]
  assert.equal(hop.url.toString(), 'https://provider.example/usage?period=week')
  assert.equal(hop.method, 'POST')
  assert.equal(hop.body.toString(), '{"x":1}')
  assert.deepEqual(hop.headers, { accept: 'application/json', authorization: `Bearer ${SECRET}` })
  assert.ok(signal instanceof AbortSignal)
  assert.equal(f.calls.auth[0].model.id, 'session-a')
})

test('路径和认证头输入拒绝绝对 URL、遍历、编码路径、控制符和未知字段', () => {
  for (const path of [
    'https://evil.example/x',
    '//evil.example/x',
    '/../x',
    '/a/../x',
    '/%2e%2e/x',
    '/a%2fb',
    '/a\\b',
    '/x#hash',
    '/x\n',
  ])
    assert.throws(() => parseProviderRequest({ path }), path)
  for (const input of [
    { path: '/usage', headers: { Authorization: 'override' } },
    { path: '/usage', headers: { cookie: 'x' } },
    { path: '/usage', auth: 'raw' },
    { path: '/usage', auth: ['bearer'] },
    { path: '/usage', url: 'https://evil.example' },
    { path: '/usage', sessionId: 'other' },
  ])
    assert.throws(() => parseProviderRequest(input))
})

test('manifest 强制明确路径和方法，不接受旧额度能力', () => {
  const base = { runtime: 'native', name: 'Test', entry: 'component.js' }
  for (const cap of [
    { reason: 'x' },
    { paths: ['/usage'] },
    { paths: ['/*'], methods: ['GET'] },
    { paths: ['/a/../usage'], methods: ['GET'] },
    { paths: ['/usage'], methods: ['TRACE'] },
  ])
    assert.equal(
      normalizeNativeManifest('test', { ...base, capabilities: { 'provider.request': cap } })
        .manifest,
      null,
    )
  assert.equal(
    normalizeNativeManifest('test', {
      ...base,
      capabilities: { 'providerUsage.read': { reason: 'old' } },
    }).manifest,
    null,
  )
})

test('认证按协议或显式格式注入，解析后的配置头覆盖默认值', async () => {
  for (const [api, header] of [
    ['anthropic-messages', 'x-api-key'],
    ['google-generative-ai', 'x-goog-api-key'],
    ['openai-responses', 'authorization'],
  ]) {
    const f = fixture({ model: { api } })
    await request(f)
    assert.equal(
      f.calls.send[0].hop.headers[header],
      header === 'authorization' ? `Bearer ${SECRET}` : SECRET,
    )
  }
  const f = fixture({ model: { api: 'anthropic-messages' } })
  await request(f, { path: '/usage', auth: 'bearer' })
  assert.deepEqual(f.calls.send[0].hop.headers, {
    authorization: `Bearer ${SECRET}`,
    'anthropic-version': '2023-06-01',
  })
  const configured = fixture({
    auth: {
      apiKey: '',
      headers: { 'X-Api-Key': 'configured-credential', 'anthropic-version': '2023-06-01' },
    },
  })
  await request(configured)
  assert.equal(configured.calls.send[0].hop.headers['x-api-key'], 'configured-credential')
})

test('Azure 使用 api-key，Anthropic OAuth 使用 Bearer 且自动补协议版本', async () => {
  for (const api of ['azure-openai-responses', 'openai-completions']) {
    const f = fixture({ model: { provider: 'azure', api } })
    await request(f)
    assert.equal(f.calls.send[0].hop.headers['api-key'], SECRET)
    assert.equal(f.calls.send[0].hop.headers.authorization, undefined)
  }

  const apiKey = fixture({ model: { provider: 'anthropic', api: 'anthropic-messages' } })
  await request(apiKey)
  assert.equal(apiKey.calls.send[0].hop.headers['x-api-key'], SECRET)
  assert.equal(apiKey.calls.send[0].hop.headers['anthropic-version'], '2023-06-01')

  const oauth = fixture({
    model: { provider: 'anthropic', api: 'anthropic-messages' },
    auth: { apiKey: 'sk-ant-oat-fixture-credential' },
  })
  await request(oauth)
  assert.equal(
    oauth.calls.send[0].hop.headers.authorization,
    'Bearer sk-ant-oat-fixture-credential',
  )
  assert.equal(oauth.calls.send[0].hop.headers['x-api-key'], undefined)
  assert.equal(oauth.calls.send[0].hop.headers['anthropic-version'], '2023-06-01')
})

test('Provider 请求只能使用宿主已同步的当前会话', async () => {
  const f = fixture()
  await assert.rejects(f.service.request('widget', 'session-b', { path: '/usage' }), {
    code: 'component_provider_session_mismatch',
  })
  assert.equal(f.calls.auth.length, 0)
  assert.equal(f.calls.send.length, 0)
  f.service.selectSession('session-b')
  await f.service.request('widget', 'session-b', { path: '/usage' })
  assert.equal(f.calls.auth[0].model.id, 'session-b')
})

test('无认证、未知签名协议、危险配置地址或跨 origin 覆盖不会发送凭据', async () => {
  for (const options of [
    { auth: { apiKey: '' } },
    { model: { api: 'bedrock-converse-stream' } },
    { auth: { baseUrl: 'https://other.example/v1' } },
    { model: { baseUrl: 'http://provider.example' } },
    { model: { baseUrl: 'https://127.0.0.1' } },
    { auth: { headers: { host: 'evil.example' } } },
    { auth: { headers: { authorization: null } } },
    { auth: { apiKey: 'bad\r\nkey' } },
  ]) {
    const f = fixture(options)
    await assert.rejects(request(f))
    assert.equal(f.calls.send.length, 0)
  }
  const missing = fixture({ getSession: async () => null })
  await assert.rejects(request(missing), { code: 'component_provider_session_unavailable' })
  assert.equal(missing.calls.auth.length, 0)
})

test('超时覆盖会话恢复和认证，不遗留后续请求', async () => {
  // 保持事件循环存活，超时来自实现中的 AbortSignal，不靠 sleep 猜测竞态。
  const keepAlive = setInterval(() => {}, 1000)
  try {
    for (const stage of ['getSession', 'getAuth']) {
      const f = fixture({ [stage]: async () => new Promise(() => {}), timeoutMs: 20 })
      await assert.rejects(request(f), { code: 'component_provider_timeout' })
      assert.equal(f.calls.send.length, 0)
      assert.equal(f.transport.limiter.buckets.get('widget').active, 0)
    }
  } finally {
    clearInterval(keepAlive)
  }
})

test('取消认证和限流覆盖全部请求生命周期，异常不包含凭据', async () => {
  let started
  const start = new Promise((resolve) => {
    started = resolve
  })
  const f = fixture({
    getAuth: async () => {
      started()
      return new Promise(() => {})
    },
    concurrent: 1,
  })
  const controller = new AbortController()
  const pending = request(f, undefined, controller.signal)
  await start
  await assert.rejects(request(f), { code: 'component_fetch_rate_limited' })
  controller.abort()
  await assert.rejects(pending, { code: 'component_request_aborted' })
  assert.equal(f.calls.send.length, 0)
  const error = fixture({
    getAuth: async () => {
      throw new Error(SECRET)
    },
  })
  await assert.rejects(
    request(error),
    (err) =>
      err.code === 'component_provider_request_failed' && !JSON.stringify(err).includes(SECRET),
  )
})

test('上游回显的原始凭据被拒绝，包括二进制响应和响应头', async () => {
  for (const response of [
    result(JSON.stringify({ token: SECRET })),
    { ...result(), body: Buffer.from(SECRET).toString('base64'), bodyEncoding: 'base64' },
    { ...result(), headers: { etag: SECRET } },
  ])
    await assert.rejects(request(fixture({ result: response })), {
      code: 'component_provider_sensitive_response',
    })
})

test('真实 TLS 传输固定已校验地址、过滤头、拒绝重定向和超大响应', async (t) => {
  const pems = await selfsigned.generate([{ name: 'commonName', value: 'provider.example' }], {
    keySize: 2048,
    extensions: [
      { name: 'basicConstraints', cA: true },
      { name: 'subjectAltName', altNames: [{ type: 2, value: 'provider.example' }] },
    ],
  })
  const seen = []
  const server = createServer({ key: pems.private, cert: pems.cert }, (req, res) => {
    seen.push(req.headers)
    if (req.url === '/redirect') {
      res.writeHead(302, { location: 'https://evil.example/steal' })
      res.end()
      return
    }
    if (req.url === '/large') {
      res.writeHead(200, { 'content-type': 'application/json', 'content-length': 500 })
      res.end('x'.repeat(500))
      return
    }
    res.writeHead(200, {
      'content-type': 'application/json',
      'set-cookie': 'private=1',
      'x-ratelimit-remaining': '75',
    })
    res.end('{"remaining":75}')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  t.after(() => {
    server.closeAllConnections()
    return new Promise((resolve) => server.close(resolve))
  })
  const transport = new ComponentFetchProxy({
    lookup: async () => [{ address: '127.0.0.1', family: 4 }],
    isBlockedAddress: () => false,
    port: server.address().port,
    ca: pems.cert,
    maxResponseBytes: 100,
  })
  const f = fixture({ transport })
  const response = await request(f)
  assert.equal(response.headers['x-ratelimit-remaining'], '75')
  assert.equal(response.headers['set-cookie'], undefined)
  assert.equal(seen[0].authorization, `Bearer ${SECRET}`)
  await assert.rejects(request(f, { path: '/redirect' }), {
    code: 'component_provider_redirect_denied',
  })
  assert.equal(seen.length, 2)
  await assert.rejects(request(f, { path: '/large' }), { code: 'component_fetch_too_large' })
  const blocked = new ComponentFetchProxy({
    lookup: async () => [{ address: '127.0.0.1', family: 4 }],
  })
  await assert.rejects(request(fixture({ transport: blocked })), {
    code: 'component_fetch_address_blocked',
  })
})

test('TUN 合成地址只触发无凭据公网解析，私网与混合地址不走回退', async () => {
  const calls = []
  const signal = new AbortController().signal
  const question = [{ name: 'provider.example.', type: 1 }]
  const fetchImpl = async (url, init) => {
    calls.push({ url, init })
    return new Response(
      JSON.stringify({ Status: 0, Question: question, Answer: [{ type: 1, data: '1.1.1.1' }] }),
    )
  }
  const lookup = async () => [{ address: '198.18.0.2', family: 4 }]
  assert.deepEqual(
    await lookupProviderAddresses('provider.example', signal, { lookup, fetchImpl }),
    [{ address: '1.1.1.1', family: 4 }],
  )
  assert.equal(calls[0].url.hostname, 'cloudflare-dns.com')
  assert.equal(calls[0].url.searchParams.get('name'), 'provider.example')
  assert.deepEqual(calls[0].init.headers, { accept: 'application/dns-json' })
  assert.equal(calls[0].init.credentials, 'omit')
  assert.equal(calls[0].init.redirect, 'error')
  assert.equal(calls[0].init.signal, signal)
  for (const addresses of [
    [{ address: '10.0.0.2', family: 4 }],
    [{ address: '1.1.1.1', family: 4 }],
    [
      { address: '198.18.0.2', family: 4 },
      { address: '127.0.0.1', family: 4 },
    ],
  ])
    assert.deepEqual(
      await lookupProviderAddresses('provider.example', signal, {
        lookup: async () => addresses,
        fetchImpl,
      }),
      addresses,
    )
  assert.equal(calls.length, 1)
  for (const payload of [
    { Status: 0, Question: question, Answer: [{ type: 1, data: '127.0.0.1' }] },
    { Status: 0, Question: question, Answer: [{ type: 1, data: '198.18.0.2' }] },
    {
      Status: 0,
      Question: [{ name: 'other.example.', type: 1 }],
      Answer: [{ type: 1, data: '1.1.1.1' }],
    },
    { Status: 0, TC: true, Question: question, Answer: [{ type: 1, data: '1.1.1.1' }] },
    { Status: 3 },
  ])
    await assert.rejects(
      lookupProviderAddresses('provider.example', signal, {
        lookup,
        fetchImpl: async () => new Response(JSON.stringify(payload)),
      }),
    )
  await assert.rejects(
    lookupProviderAddresses('provider.example', signal, {
      lookup,
      fetchImpl: async () => new Response('x'.repeat(65537)),
    }),
  )
})
