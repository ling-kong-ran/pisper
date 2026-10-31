// 组件网络代理（SSRF 防护）按负例逐项覆盖：本地 TLS 服务（selfsigned）+ 注入 lookup / ca / 地址策略。
// 覆盖：http 被拒、端口 / userinfo / IP 字面量、未声明主机、DNS 解析到回环 / 私网 / 链路本地 / IPv6
// 映射 / ULA / 链路本地 / CGNAT、多地址里混一个私网、重定向到未声明主机或私网、重定向上限、超时、
// 超过 2 MB、请求头与响应头白名单、不转发应用的 Cookie 与 Authorization、限流、统计不含查询参数。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { createServer as createHttpServer } from 'node:http'
import { createServer as createHttpsServer } from 'node:https'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import selfsigned from 'selfsigned'
import { authorizeDesktopRequest } from '../desktop-sidecar-auth.mjs'
import { createApiHandler } from '../http/api-handler.mjs'
import { isBlockedAddress, isBlockedHostname, mappedIpv4 } from '../security/public-address.mjs'
import { CustomUiService } from '../services/custom-ui-service.mjs'
import {
  ComponentFetchProxy,
  checkFetchTarget,
  hostMatches,
} from '../services/custom-ui-fetch-proxy.mjs'

const HOSTS = ['api.example.test', 'other.example.test', 'private.example.test', '*.wild.test']
const NETWORK = { hosts: HOSTS, reason: '' }

let tls = null
async function tlsFixture() {
  if (tls) return tls
  const pems = await selfsigned.generate([{ name: 'commonName', value: 'api.example.test' }], {
    days: 1,
    keySize: 2048,
    algorithm: 'sha256',
    extensions: [
      { name: 'basicConstraints', cA: true },
      {
        name: 'subjectAltName',
        altNames: [
          { type: 2, value: 'api.example.test' },
          { type: 2, value: 'other.example.test' },
          { type: 2, value: 'private.example.test' },
          { type: 2, value: 'a.wild.test' },
        ],
      },
    ],
  })
  tls = { key: pems.private, cert: pems.cert }
  return tls
}

/** 本地 TLS 上游：按路径返回不同行为，并记录收到的请求（用来断言“根本没连上”）。 */
async function startUpstream(t) {
  const { key, cert } = await tlsFixture()
  const requests = []
  const server = createHttpsServer({ key, cert }, (req, res) => {
    const url = new URL(req.url, 'https://upstream')
    requests.push({
      path: url.pathname,
      query: url.search,
      method: req.method,
      headers: req.headers,
    })
    let body = ''
    req.on('data', (chunk) => (body += chunk))
    req.on('end', () => {
      switch (url.pathname) {
        case '/echo':
          res.writeHead(200, {
            'content-type': 'application/json; charset=utf-8',
            'set-cookie': 'upstream=1',
            server: 'fixture',
            'x-ratelimit-remaining': '42',
            etag: '"v1"',
            'access-control-allow-origin': '*',
          })
          res.end(JSON.stringify({ headers: req.headers, method: req.method, body }))
          return
        case '/binary':
          res.writeHead(200, { 'content-type': 'image/png' })
          res.end(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0, 1, 2, 255]))
          return
        case '/to-other':
          res.writeHead(302, { location: 'https://other.example.test/echo' })
          res.end()
          return
        case '/to-undeclared':
          res.writeHead(302, { location: 'https://undeclared.test/echo' })
          res.end()
          return
        case '/to-private':
          res.writeHead(307, { location: 'https://private.example.test/echo' })
          res.end()
          return
        case '/to-http':
          res.writeHead(301, { location: 'http://api.example.test/echo' })
          res.end()
          return
        case '/to-ip':
          res.writeHead(301, { location: 'https://127.0.0.1/echo' })
          res.end()
          return
        case '/loop':
          res.writeHead(302, { location: '/loop' })
          res.end()
          return
        case '/post-303':
          res.writeHead(303, { location: '/echo' })
          res.end()
          return
        case '/slow':
          // 永不响应：靠代理的超时收场；服务器关闭时 closeAllConnections 清理。
          return
        case '/big': {
          res.writeHead(200, { 'content-type': 'application/octet-stream' })
          const chunk = Buffer.alloc(256 * 1024, 1)
          let sent = 0
          const pump = () => {
            while (sent < 12) {
              sent += 1
              if (!res.write(chunk)) return res.once('drain', pump)
            }
            res.end()
          }
          pump()
          return
        }
        case '/big-declared':
          res.writeHead(200, { 'content-type': 'text/plain', 'content-length': 3 * 1024 * 1024 })
          res.end()
          return
        default:
          res.writeHead(404, { 'content-type': 'text/plain' })
          res.end('missing')
      }
    })
  })
  await new Promise((resolveListen) => server.listen(0, '127.0.0.1', resolveListen))
  t.after(
    () =>
      new Promise((resolveClose) => {
        server.closeAllConnections()
        server.close(() => resolveClose())
      }),
  )
  return { port: server.address().port, cert, requests }
}

const DNS = {
  'api.example.test': ['127.0.0.1'],
  'other.example.test': ['127.0.0.1'],
  'a.wild.test': ['127.0.0.1'],
  'private.example.test': ['10.0.0.8'],
}

/**
 * 测试用代理：上游只能是本机 TLS，所以地址策略放行 127.0.0.1，其余仍按默认阻断表。
 * 负例用 strict:true（完全默认的阻断表）。
 */
function proxyFor(upstream, { dns = DNS, strict = false, ...options } = {}) {
  const lookups = []
  const proxy = new ComponentFetchProxy({
    lookup: async (hostname) => {
      lookups.push(hostname)
      const addresses = dns[hostname]
      if (!addresses) throw Object.assign(new Error('ENOTFOUND'), { code: 'ENOTFOUND' })
      return addresses.map((address) => ({ address, family: address.includes(':') ? 6 : 4 }))
    },
    isBlockedAddress: strict
      ? (address) => isBlockedAddress(address)
      : (address) => address !== '127.0.0.1' && isBlockedAddress(address),
    ca: upstream.cert,
    port: upstream.port,
    ...options,
  })
  return { proxy, lookups }
}

async function rejectsCode(promise, code, status) {
  await assert.rejects(promise, (error) => {
    assert.equal(error.code, code)
    if (status) assert.equal(error.statusCode, status)
    return true
  })
}

test('地址判定：IPv4 / IPv6 私网、回环、链路本地、映射地址与元数据主机名都阻断，公网放行', () => {
  for (const address of [
    '0.0.0.0',
    '10.1.2.3',
    '100.64.0.1',
    '127.0.0.1',
    '127.255.255.254',
    '169.254.169.254',
    '172.16.0.1',
    '172.31.255.255',
    '192.0.0.1',
    '192.168.1.1',
    '198.18.0.1',
    '224.0.0.1',
    '240.0.0.1',
    '255.255.255.255',
    '::',
    '::1',
    '::ffff:127.0.0.1',
    '::ffff:7f00:1',
    '::ffff:10.0.0.1',
    '::ffff:169.254.169.254',
    '0:0:0:0:0:ffff:c0a8:0101',
    'fc00::1',
    'fd12:3456::1',
    'fe80::1',
    'fe80::1%eth0',
    'ff02::1',
    '64:ff9b::a00:1',
    '2002:7f00:1::1',
    'not-an-ip',
    '',
  ])
    assert.equal(isBlockedAddress(address), true, address)
  for (const address of ['8.8.8.8', '93.184.216.34', '::ffff:8.8.8.8', '2606:4700:4700::1111'])
    assert.equal(isBlockedAddress(address), false, address)
  assert.equal(mappedIpv4('::ffff:7f00:1'), '127.0.0.1')
  assert.equal(mappedIpv4('2606:4700::1'), null)
  for (const host of [
    'localhost',
    'a.localhost',
    'metadata',
    'metadata.google.internal',
    'printer.local',
    'corp.internal',
  ])
    assert.equal(isBlockedHostname(host), true, host)
  assert.equal(isBlockedHostname('api.github.com'), false)
})

test('目标 URL：只许 https、443、无 userinfo、非 IP 字面量、已声明主机；通配只匹配一级子域', () => {
  const invalid = (url) =>
    assert.throws(
      () => checkFetchTarget(url, HOSTS),
      (error) => error.code === 'component_fetch_invalid' && error.statusCode === 400,
      url,
    )
  invalid('http://api.example.test/echo')
  invalid('ftp://api.example.test/echo')
  invalid('https://api.example.test:8443/echo')
  invalid('https://user:pass@api.example.test/echo')
  invalid('https://127.0.0.1/echo')
  invalid('https://2130706433/echo')
  invalid('https://0x7f.1/echo')
  invalid('https://[::1]/echo')
  invalid('https://[::ffff:127.0.0.1]/echo')
  invalid('https://api.example.test./echo')
  invalid(`https://api.example.test/${'a'.repeat(2100)}`)
  invalid('not a url')
  invalid(42)
  assert.throws(
    () => checkFetchTarget('https://undeclared.test/', HOSTS),
    (error) => error.code === 'component_fetch_host_denied' && error.statusCode === 403,
  )
  // 即使 manifest 声明了元数据主机名，也不放行。
  assert.throws(
    () => checkFetchTarget('https://metadata.google.internal/', ['metadata.google.internal']),
    (error) => error.code === 'component_fetch_address_blocked',
  )
  assert.equal(
    checkFetchTarget('https://API.example.test:443/x?y#z', HOSTS).toString(),
    'https://api.example.test/x?y',
  )
  assert.equal(hostMatches(['*.wild.test'], 'a.wild.test'), true)
  assert.equal(hostMatches(['*.wild.test'], 'wild.test'), false)
  assert.equal(hostMatches(['*.wild.test'], 'a.b.wild.test'), false)
  assert.equal(hostMatches(['*.wild.test'], 'evilwild.test'), false)
})

test('没有声明 network 能力：403 component_capability_denied，不发请求', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy, lookups } = proxyFor(upstream)
  await rejectsCode(
    proxy.fetch('demo', undefined, { url: 'https://api.example.test/echo' }),
    'component_capability_denied',
    403,
  )
  await rejectsCode(
    proxy.fetch('demo', { hosts: [] }, { url: 'https://api.example.test/echo' }),
    'component_capability_denied',
    403,
  )
  assert.deepEqual(lookups, [])
  assert.equal(upstream.requests.length, 0)
})

test('成功请求：不带任何应用凭据，固定 UA，请求头与响应头按白名单，JSON 以 utf8 返回', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy } = proxyFor(upstream)
  const result = await proxy.fetch('demo', NETWORK, {
    url: 'https://api.example.test/echo?token=secret',
    method: 'post',
    headers: {
      Accept: 'application/json',
      authorization: 'Bearer component-own',
      'content-type': 'text/plain',
    },
    body: '你好',
  })
  assert.equal(result.status, 200)
  assert.equal(result.bodyEncoding, 'utf8')
  assert.equal(result.redirected, false)
  assert.equal(result.url, 'https://api.example.test/echo?token=secret')
  const echoed = JSON.parse(result.body)
  assert.equal(echoed.method, 'POST')
  assert.equal(echoed.body, '你好')
  assert.equal(echoed.headers['user-agent'], 'Pisper-Component/1')
  assert.equal(echoed.headers.authorization, 'Bearer component-own')
  assert.equal(echoed.headers.cookie, undefined)
  assert.equal(echoed.headers.origin, undefined)
  assert.equal(echoed.headers['accept-encoding'], 'identity')
  assert.deepEqual(Object.keys(result.headers).sort(), [
    'content-type',
    'date',
    'etag',
    'x-ratelimit-remaining',
  ])
  // 统计只记主机，不记查询参数。
  const stats = proxy.networkStats('demo')
  assert.equal(stats.count, 1)
  assert.deepEqual(stats.hosts, { 'api.example.test': 1 })
  assert.doesNotMatch(JSON.stringify(stats), /secret|echo/)
})

test('二进制响应以 base64 返回', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy } = proxyFor(upstream)
  const result = await proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/binary' })
  assert.equal(result.bodyEncoding, 'base64')
  assert.deepEqual([...Buffer.from(result.body, 'base64')], [0x89, 0x50, 0x4e, 0x47, 0, 1, 2, 255])
})

test('请求校验：未知字段、未列入白名单的请求头、CR/LF 注入、GET 带 body、坏 base64 都是 400', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy } = proxyFor(upstream)
  const url = 'https://api.example.test/echo'
  for (const input of [
    null,
    { url, extra: 1 },
    { url, method: 'TRACE' },
    { url, headers: { cookie: 'a=1' } },
    { url, headers: { origin: 'https://evil' } },
    { url, headers: { host: 'internal' } },
    { url, headers: { accept: 'a\r\nx-injected: 1' } },
    { url, headers: { accept: 1 } },
    { url, body: 'x' },
    { url, method: 'POST', body: '***', bodyEncoding: 'base64' },
    { url, method: 'POST', body: 'x', bodyEncoding: 'latin1' },
    { url, method: 'POST', body: 'x'.repeat(400 * 1024) },
  ])
    await rejectsCode(proxy.fetch('demo', NETWORK, input), 'component_fetch_invalid', 400)
  assert.equal(upstream.requests.length, 0)
})

test('DNS 解析到回环 / 私网 / 链路本地 / CGNAT / IPv6 回环 / 映射 / ULA / 链路本地：403，不连接', async (t) => {
  const upstream = await startUpstream(t)
  for (const address of [
    '127.0.0.1',
    '10.0.0.1',
    '169.254.169.254',
    '100.64.0.1',
    '172.16.5.4',
    '192.168.0.10',
    '::1',
    '::ffff:127.0.0.1',
    'fc00::1',
    'fe80::1',
  ]) {
    const { proxy } = proxyFor(upstream, { strict: true, dns: { 'api.example.test': [address] } })
    await rejectsCode(
      proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/echo' }),
      'component_fetch_address_blocked',
      403,
    )
  }
  // 多个地址里只要有一个被阻断就整体拒绝（不挑“好的”那个连）。
  const { proxy } = proxyFor(upstream, {
    strict: true,
    dns: { 'api.example.test': ['93.184.216.34', '10.0.0.1'] },
  })
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/echo' }),
    'component_fetch_address_blocked',
    403,
  )
  assert.equal(upstream.requests.length, 0)
})

test('连接只用校验过的地址：lookup 只调用一次，没有第二次解析的机会（防 DNS rebinding）', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy, lookups } = proxyFor(upstream)
  await proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/echo' })
  assert.deepEqual(lookups, ['api.example.test'])
})

test('重定向：跨主机重新校验并去掉 Authorization；到未声明主机、私网、http、IP 字面量都拒绝', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy, lookups } = proxyFor(upstream)
  const result = await proxy.fetch('demo', NETWORK, {
    url: 'https://api.example.test/to-other',
    headers: { authorization: 'Bearer component-own' },
  })
  assert.equal(result.redirected, true)
  assert.equal(result.url, 'https://other.example.test/echo')
  assert.equal(JSON.parse(result.body).headers.authorization, undefined)
  assert.deepEqual(lookups, ['api.example.test', 'other.example.test'])

  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/to-undeclared' }),
    'component_fetch_host_denied',
    403,
  )
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/to-private' }),
    'component_fetch_address_blocked',
    403,
  )
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/to-http' }),
    'component_fetch_invalid',
    400,
  )
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/to-ip' }),
    'component_fetch_invalid',
    400,
  )
  assert.equal(upstream.requests.filter((request) => request.path === '/echo').length, 1)
})

test('重定向超过 5 跳：502；303 把 POST 改成 GET 并丢掉请求体', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy } = proxyFor(upstream)
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/loop' }),
    'component_fetch_redirect_limit',
    502,
  )
  assert.equal(upstream.requests.filter((request) => request.path === '/loop').length, 6)
  const result = await proxy.fetch('demo', NETWORK, {
    url: 'https://api.example.test/post-303',
    method: 'POST',
    body: 'payload',
  })
  const echoed = JSON.parse(result.body)
  assert.equal(echoed.method, 'GET')
  assert.equal(echoed.body, '')
})

test('超时：504 component_fetch_timeout', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy } = proxyFor(upstream, { timeoutMs: 200 })
  const started = Date.now()
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/slow' }),
    'component_fetch_timeout',
    504,
  )
  assert.ok(Date.now() - started < 5000)
})

test('响应超过 2 MiB：流式超限与声明超限都立即中断，502 component_fetch_too_large', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy } = proxyFor(upstream)
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/big' }),
    'component_fetch_too_large',
    502,
  )
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/big-declared' }),
    'component_fetch_too_large',
    502,
  )
})

test('限流：每个组件每分钟 30 次，超出 429；组件之间互不影响；时间推进后恢复', async (t) => {
  const upstream = await startUpstream(t)
  let now = 1_000_000
  const { proxy } = proxyFor(upstream, { now: () => now, perMinute: 3 })
  for (let index = 0; index < 3; index += 1)
    await proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/echo' })
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/echo' }),
    'component_fetch_rate_limited',
    429,
  )
  await proxy.fetch('other', NETWORK, { url: 'https://api.example.test/echo' })
  now += 20_000
  await proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/echo' })
})

test('并发上限：同一组件同时最多 4 个请求', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy } = proxyFor(upstream, { timeoutMs: 300 })
  const slow = Array.from({ length: 4 }, () =>
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/slow' }).catch((error) => error),
  )
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/echo' }),
    'component_fetch_rate_limited',
    429,
  )
  for (const error of await Promise.all(slow)) assert.equal(error.code, 'component_fetch_timeout')
})

test('上游不可解析：502 component_fetch_failed', async (t) => {
  const upstream = await startUpstream(t)
  const { proxy } = proxyFor(upstream, { dns: {} })
  await rejectsCode(
    proxy.fetch('demo', NETWORK, { url: 'https://api.example.test/echo' }),
    'component_fetch_failed',
    502,
  )
})

test('HTTP 路由：经鉴权链进入；应用请求里的 Cookie 与 Authorization 不会转发给上游；停用与未声明都拒绝', async (t) => {
  const upstream = await startUpstream(t)
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-fetch-'))
  const { proxy } = proxyFor(upstream)
  const service = new CustomUiService({ dataDir: directory, fetchProxy: proxy })
  service.start({ hostVersion: '0.6.5' })
  const write = async (id, manifest) => {
    await mkdir(join(service.root, id), { recursive: true })
    await writeFile(join(service.root, id, 'manifest.json'), JSON.stringify(manifest))
    await writeFile(join(service.root, id, 'component.js'), 'export default () => null\n')
  }
  await write('net', {
    runtime: 'native',
    name: 'Net',
    capabilities: { network: { hosts: ['api.example.test'], reason: 'demo' } },
  })
  await write('plain', { runtime: 'native', name: 'Plain' })
  const handler = createApiHandler({}, { customUi: service })
  const server = createHttpServer(async (req, res) => {
    const origin = `http://127.0.0.1:${server.address().port}`
    const url = new URL(req.url, origin)
    if (authorizeDesktopRequest(req, res, url, { token: 'fixture', origin })) return
    await handler(req, res, url)
  })
  await new Promise((resolveListen) => server.listen(0, '127.0.0.1', resolveListen))
  t.after(async () => {
    service.dispose()
    await new Promise((resolveClose) => server.close(() => resolveClose()))
    await rm(directory, { recursive: true, force: true })
  })
  const origin = `http://127.0.0.1:${server.address().port}`
  const post = (id, body, headers = {}) =>
    fetch(`${origin}/api/custom-ui/components/${id}/fetch`, {
      method: 'POST',
      headers: {
        Cookie: '__pisper_desktop=fixture',
        Origin: origin,
        'Content-Type': 'application/json',
        ...headers,
      },
      body: JSON.stringify(body),
    })
  const unauthenticated = await fetch(`${origin}/api/custom-ui/components/net/fetch`, {
    method: 'POST',
    body: '{}',
  })
  assert.equal(unauthenticated.status, 401)

  const ok = await post(
    'net',
    { url: 'https://api.example.test/echo' },
    { Authorization: 'Bearer app-secret' },
  )
  assert.equal(ok.status, 200)
  const payload = await ok.json()
  const echoed = JSON.parse(payload.body)
  assert.equal(echoed.headers.cookie, undefined)
  assert.equal(echoed.headers.authorization, undefined)
  assert.doesNotMatch(payload.body, /fixture|app-secret/)

  const denied = await post('plain', { url: 'https://api.example.test/echo' })
  assert.equal(denied.status, 403)
  assert.deepEqual(await denied.json(), {
    capability: 'network',
    code: 'component_capability_denied',
    error: 'component_capability_denied',
  })
  const host = await post('net', { url: 'https://other.example.test/echo' })
  assert.equal(host.status, 403)
  assert.equal((await host.json()).code, 'component_fetch_host_denied')

  await service.setEnabled('net', { enabled: false })
  const disabled = await post('net', { url: 'https://api.example.test/echo' })
  assert.equal(disabled.status, 409)
  assert.equal((await disabled.json()).code, 'component_disabled')

  const errors = await fetch(`${origin}/api/custom-ui/components/net/errors`, {
    headers: { Cookie: '__pisper_desktop=fixture' },
  })
  const listed = await errors.json()
  // 统计只计真正发往网络的请求：被拒的主机不计入。
  assert.equal(listed.network.count, 1)
  assert.deepEqual(listed.network.hosts, { 'api.example.test': 1 })
  assert.equal(upstream.requests.length, 1)
})
