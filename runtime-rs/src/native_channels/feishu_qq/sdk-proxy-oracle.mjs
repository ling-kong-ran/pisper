// Actual SDK proxy oracle and a strictly owned-loopback fixture server.
import fs from 'node:fs'
import http from 'node:http'
import https from 'node:https'
import net from 'node:net'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads'
import { gzipSync, deflateSync, deflateRawSync, brotliCompressSync, zstdCompressSync } from 'node:zlib'
const require = createRequire(import.meta.url)
const directory = path.dirname(fileURLToPath(import.meta.url))
const certPath = path.join(directory, 'proxy-test-cert.pem')
const keyPath = path.join(directory, 'proxy-test-key.pem')
const proxyNames = ['http_proxy', 'HTTP_PROXY', 'https_proxy', 'HTTPS_PROXY', 'all_proxy', 'ALL_PROXY', 'no_proxy', 'NO_PROXY']

if (!isMainThread) {
  const sdk = require('../../../../node_modules/@larksuiteoapi/node-sdk')
  const { getProxyForUrl } = require('../../../../node_modules/proxy-from-env')
  const { default: shouldBypassProxy } = await import('../../../../node_modules/axios/lib/helpers/shouldBypassProxy.js')
  for (const item of workerData.cases) {
    for (const key of proxyNames) delete process.env[key]
    Object.assign(process.env, item.expandedEnv)
    const channel = sdk.createLarkChannel({ appId: 'cli_0123456789abcdef', appSecret: 'synthetic-only', logger: { trace() {}, debug() {}, info() {}, warn() {}, error() {} }, loggerLevel: sdk.LoggerLevel.error, outbound: { ssrfGuard: { allowlist: ['127.0.0.1', 'localhost'] } } })
    const candidates = item.selectionUrls.map(url => ({ url, proxy: shouldBypassProxy(url) ? '' : getProxyForUrl(url) }))
    parentPort.postMessage({ type: 'start' })
    await new Promise(resolve => parentPort.once('message', resolve))
    let result
    try {
      const body = await channel.sender.uploader.toBuffer(item.expandedSource)
      result = { body: body.toString(), error: null }
    } catch (error) {
      result = { body: null, error: { code: error.code, message: error.message, cause: { code: error.cause?.code, message: error.cause?.message } } }
    } finally { await channel.safety.dispose() }
    parentPort.postMessage({ type: 'result', name: item.name, result, candidates })
    await new Promise(resolve => parentPort.once('message', resolve))
  }
  parentPort.postMessage({ type: 'done' })
} else {
  const tls = { key: fs.readFileSync(keyPath), cert: fs.readFileSync(certPath) }
  const records = []
  const endpoints = {}
  const servers = [], sockets = new Set()
  const allowedHosts = new Set(['127.0.0.1', 'localhost', 'redirect.invalid'])
  const allowedPorts = () => new Set(Object.values(endpoints).map(url => Number(new URL(url).port)))
  const origin = id => (request, response) => {
    if (request.url === '/__records') { response.setHeader('Content-Type', 'application/json'); response.end(JSON.stringify(records.splice(0))); return }
    records.push({ server: id, method: request.method, path: request.url, host: request.headers.host, proxyAuthorization: request.headers['proxy-authorization'] ?? null })
    const url = new URL(request.url, endpoints[id])
    if (url.pathname === '/slow') return
    if (url.pathname.startsWith('/body/')) {
      const encoding = url.pathname.slice(6)
      const buffer = ['gzip', 'x-gzip'].includes(encoding) ? gzipSync('synthetic proxied media') : ['deflate', 'compress', 'x-compress'].includes(encoding) ? deflateSync('synthetic proxied media') : encoding === 'raw-deflate' ? deflateRawSync('synthetic proxied media') : encoding === 'zstd' ? zstdCompressSync('synthetic proxied media') : brotliCompressSync('synthetic proxied media')
      response.setHeader('Content-Encoding', encoding === 'raw-deflate' ? 'deflate' : encoding)
      if (url.searchParams.has('chunked')) { response.write(buffer.subarray(0, 3)); response.end(buffer.subarray(3)) }
      else response.end(buffer)
      return
    }
    if (url.pathname === '/redirect') { response.writeHead(302, { Location: url.searchParams.get('to') }); response.end(); return }
    response.end('synthetic proxied media')
  }
  const forward = id => (request, response) => {
    let target
    try { target = new URL(request.url) } catch { response.writeHead(400); response.end(); return }
    records.push({ server: id, method: request.method, target: request.url, host: request.headers.host, proxyAuthorization: request.headers['proxy-authorization'] ?? null })
    if (!allowedHosts.has(target.hostname) || !allowedPorts().has(Number(target.port)) || !['http:', 'https:'].includes(target.protocol)) { response.writeHead(403); response.end('only owned fixture destinations allowed'); return }
    const headers = { ...request.headers }; delete headers['proxy-authorization']
    const client = target.protocol === 'https:' ? https : http
    const forwarded = client.request({ hostname: '127.0.0.1', port: target.port, path: target.pathname + target.search, method: request.method, headers, ca: tls.cert, servername: target.hostname }, upstream => { response.writeHead(upstream.statusCode, upstream.headers); upstream.pipe(response) })
    forwarded.on('error', () => { response.writeHead(502); response.end('owned forwarding failed') })
    request.pipe(forwarded)
  }
  const connect = id => (request, socket, head) => {
    let target
    try { target = new URL('https://' + request.url) } catch { socket.end('HTTP/1.1 400 Bad Request\r\n\r\n'); return }
    records.push({ server: id, method: 'CONNECT', target: request.url, proxyAuthorization: request.headers['proxy-authorization'] ?? null })
    if (!allowedHosts.has(target.hostname) || !allowedPorts().has(Number(target.port))) { socket.end('HTTP/1.1 403 Forbidden\r\n\r\n'); return }
    const upstream = net.connect({ host: '127.0.0.1', port: target.port }, () => { socket.write('HTTP/1.1 200 Connection Established\r\n\r\n'); if (head.length) upstream.write(head); socket.pipe(upstream); upstream.pipe(socket) })
    sockets.add(upstream); upstream.on('close', () => sockets.delete(upstream)); upstream.on('error', () => socket.destroy())
  }
  const listen = async (id, secure, handler) => {
    const server = secure ? https.createServer(tls, handler) : http.createServer(handler)
    server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)) })
    if (id.includes('PROXY')) server.on('connect', connect(id))
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
    endpoints[id] = `${secure ? 'https' : 'http'}://127.0.0.1:${server.address().port}`
    servers.push(server)
  }
  await listen('HTTP_ORIGIN', false, origin('HTTP_ORIGIN'))
  await listen('HTTPS_ORIGIN', true, origin('HTTPS_ORIGIN'))
  await listen('HTTP_PROXY', false, forward('HTTP_PROXY'))
  await listen('HTTP_PROXY_2', false, forward('HTTP_PROXY_2'))
  await listen('HTTPS_PROXY', true, forward('HTTPS_PROXY'))
  const shutdown = async () => { for (const socket of sockets) socket.destroy(); await Promise.all(servers.map(server => new Promise(resolve => server.close(resolve)))) }
  if (process.argv[2] === '--serve') {
    console.log(JSON.stringify({ endpoints, certPath }))
    process.stdin.resume(); process.stdin.once('data', shutdown); process.stdin.once('end', shutdown)
  } else {
    const ip = '@HTTP_ORIGIN@/final', dns = '@HTTP_ORIGIN_DNS@/final'
    const secureIp = '@HTTPS_ORIGIN@/final', secureDns = '@HTTPS_ORIGIN_DNS@/final'
    const redirect = (source, target) => `${source}/redirect?to=${encodeURIComponent(target)}`
    const cases = []
    const add = (name, source, env) => cases.push({ name, source, env })
    add('direct_http_ip', ip, {})
    add('direct_http_dns', dns, {})
    add('direct_https_ip', secureIp, {})
    add('direct_https_dns', secureDns, {})
    for (const [key, proxy] of [['HTTP_PROXY', '@HTTP_PROXY@'], ['http_proxy', '@HTTP_PROXY@'], ['ALL_PROXY', '@HTTP_PROXY@'], ['all_proxy', '@HTTP_PROXY@']]) {
      add(key + '_http_ip', ip, { [key]: proxy }); add(key + '_http_dns', dns, { [key]: proxy })
    }
    for (const [key, proxy] of [['HTTPS_PROXY', '@HTTP_PROXY@'], ['https_proxy', '@HTTP_PROXY@'], ['ALL_PROXY', '@HTTP_PROXY@'], ['all_proxy', '@HTTP_PROXY@']]) {
      add(key + '_https_ip', secureIp, { [key]: proxy }); add(key + '_https_dns', secureDns, { [key]: proxy })
    }
    add('http_ignores_https_proxy', ip, { HTTPS_PROXY: '@HTTP_PROXY@' })
    add('https_ignores_http_proxy', secureIp, { HTTP_PROXY: '@HTTP_PROXY@' })
    add('lowercase_http_precedence', ip, { HTTP_PROXY: '@HTTP_PROXY@', http_proxy: '@HTTP_PROXY_2@' })
    add('scheme_over_all', ip, { HTTP_PROXY: '@HTTP_PROXY_2@', ALL_PROXY: '@HTTP_PROXY@' })
    add('http_proxy_dns_agent_defect', ip, { HTTP_PROXY: '@HTTP_PROXY_DNS@' })
    add('http_via_https_proxy_agent_defect', ip, { HTTP_PROXY: '@HTTPS_PROXY@' })
    add('https_via_dns_proxy', secureDns, { HTTPS_PROXY: '@HTTP_PROXY_DNS@' })
    add('https_via_https_proxy', secureDns, { HTTPS_PROXY: '@HTTPS_PROXY@' })
    add('http_schemeless_proxy', ip, { HTTP_PROXY: '@HTTP_PROXY_AUTHORITY@' })
    add('http_proxy_auth', dns, { HTTP_PROXY: '@HTTP_PROXY_AUTH@' })
    add('https_proxy_auth', secureDns, { HTTPS_PROXY: '@HTTP_PROXY_AUTH@' })
    for (const [name, noProxy] of [['wildcard', '*'], ['exact_ip', '127.0.0.1'], ['loopback_equivalent', 'localhost'], ['port_match', '127.0.0.1:@HTTP_ORIGIN_PORT@'], ['port_mismatch', '127.0.0.1:1'], ['comma_and_spaces', 'other.invalid, localhost other2.invalid']]) add('no_proxy_' + name, ip, { HTTP_PROXY: '@HTTP_PROXY@', NO_PROXY: noProxy })
    add('lowercase_no_proxy_precedence', ip, { HTTP_PROXY: '@HTTP_PROXY@', NO_PROXY: '*', no_proxy: 'other.invalid' })
    add('no_proxy_dns_direct_defect', dns, { HTTP_PROXY: '@HTTP_PROXY@', NO_PROXY: 'localhost' })
    add('proxy_http_redirect_dns', redirect('@HTTP_ORIGIN@', '@HTTP_REDIRECT_DNS@/final'), { HTTP_PROXY: '@HTTP_PROXY@' })
    add('direct_then_proxy_dns', redirect('@HTTP_ORIGIN@', '@HTTP_REDIRECT_DNS@/final'), { HTTP_PROXY: '@HTTP_PROXY@', NO_PROXY: '127.0.0.1' })
    add('proxy_then_direct_dns_defect', redirect('@HTTP_ORIGIN@', '@HTTP_REDIRECT_DNS@/final'), { HTTP_PROXY: '@HTTP_PROXY@', NO_PROXY: '.invalid' })
    add('proxy_http_to_https', redirect('@HTTP_ORIGIN@', '@HTTPS_ORIGIN@/final'), { HTTP_PROXY: '@HTTP_PROXY@', HTTPS_PROXY: '@HTTP_PROXY@' })
    add('proxy_https_to_http_agent_defect', redirect('@HTTPS_ORIGIN@', '@HTTP_ORIGIN@/final'), { HTTP_PROXY: '@HTTP_PROXY@', HTTPS_PROXY: '@HTTP_PROXY@' })
    add('proxy_https_redirect_dns', redirect('@HTTPS_ORIGIN@', '@HTTPS_REDIRECT_DNS@/final'), { HTTPS_PROXY: '@HTTP_PROXY@' })
    add('proxy_https_to_no_proxy_retains_tunnel', redirect('@HTTPS_ORIGIN@', '@HTTPS_REDIRECT_DNS@/final'), { HTTPS_PROXY: '@HTTP_PROXY@', NO_PROXY: '.invalid' })
    add('direct_http_to_https_proxy', redirect('@HTTP_ORIGIN@', '@HTTPS_ORIGIN@/final'), { HTTPS_PROXY: '@HTTP_PROXY@' })
    add('direct_https_to_http_proxy_agent_defect', redirect('@HTTPS_ORIGIN@', '@HTTP_ORIGIN@/final'), { HTTP_PROXY: '@HTTP_PROXY@' })
    add('https_redirect_http_through_https_proxy_retains_tunnel', redirect('@HTTPS_ORIGIN@', '@HTTP_ORIGIN@/final'), { HTTPS_PROXY: '@HTTPS_PROXY@', HTTP_PROXY: '@HTTPS_PROXY@' })
    add('https_redirect_http_changes_proxy_retains_tunnel', redirect('@HTTPS_ORIGIN@', '@HTTP_ORIGIN@/final'), { HTTPS_PROXY: '@HTTP_PROXY@', HTTP_PROXY: '@HTTPS_PROXY@' })
    for (const [name, noProxy] of [['ipv4_shorthand', '127.1'], ['ipv4_hex', '127.0x1'], ['ipv6_loopback', '[::1]'], ['mapped_ipv6', '::ffff:127.0.0.1'], ['trailing_dot', 'localhost.']]) add('no_proxy_' + name, ip, { HTTP_PROXY: '@HTTP_PROXY@', NO_PROXY: noProxy })
    for (const encoding of ['gzip', 'deflate', 'br']) add('nested_' + encoding + '_body', redirect('@HTTPS_ORIGIN@', '@HTTP_ORIGIN@/body/' + encoding), { HTTPS_PROXY: '@HTTP_PROXY@', HTTP_PROXY: '@HTTPS_PROXY@' })
    add('nested_chunked_gzip_body', redirect('@HTTPS_ORIGIN@', '@HTTP_ORIGIN@/body/gzip?chunked=1'), { HTTPS_PROXY: '@HTTP_PROXY@', HTTP_PROXY: '@HTTPS_PROXY@' })
    for (const encoding of ['zstd', 'x-gzip', 'compress', 'x-compress', 'raw-deflate']) {
      add('nested_' + encoding + '_body', redirect('@HTTPS_ORIGIN@', '@HTTP_ORIGIN@/body/' + encoding), { HTTPS_PROXY: '@HTTP_PROXY@', HTTP_PROXY: '@HTTPS_PROXY@' })
      add('proxy_' + encoding + '_body', '@HTTP_ORIGIN_DNS@/body/' + encoding, { HTTP_PROXY: '@HTTP_PROXY@' })
    }
    add('extra_ca_environment', secureIp, { NODE_EXTRA_CA_CERTS: '@OWNED_CA_CERT@' })
    add('encoded_http_proxy_auth', dns, { HTTP_PROXY: '@HTTP_PROXY_AUTH_ENCODED@' })
    add('encoded_https_proxy_auth', secureDns, { HTTPS_PROXY: '@HTTP_PROXY_AUTH_ENCODED@' })
    add('password_only_proxy_has_no_auth', dns, { HTTP_PROXY: '@HTTP_PROXY_PASSWORD_ONLY@' })
    const substitutions = { ...endpoints, HTTP_ORIGIN_DNS: endpoints.HTTP_ORIGIN.replace('127.0.0.1', 'localhost'), HTTPS_ORIGIN_DNS: endpoints.HTTPS_ORIGIN.replace('127.0.0.1', 'localhost'), HTTP_REDIRECT_DNS: endpoints.HTTP_ORIGIN.replace('127.0.0.1', 'redirect.invalid'), HTTPS_REDIRECT_DNS: endpoints.HTTPS_ORIGIN.replace('127.0.0.1', 'redirect.invalid'), HTTP_PROXY_DNS: endpoints.HTTP_PROXY.replace('127.0.0.1', 'localhost'), HTTP_PROXY_AUTHORITY: endpoints.HTTP_PROXY.slice(7), HTTP_PROXY_AUTH: endpoints.HTTP_PROXY.replace('http://', 'http://synthetic:proxy-only@'), HTTP_ORIGIN_PORT: new URL(endpoints.HTTP_ORIGIN).port }
    for (const [name, url] of Object.entries(endpoints)) substitutions[name + '_PORT'] = new URL(url).port
    substitutions.OWNED_CA_CERT = path.join(directory, 'proxy-test-ca.pem')
    substitutions.HTTP_PROXY_AUTH_ENCODED = endpoints.HTTP_PROXY.replace('http://', 'http://synthetic%40user:proxy%3Aonly@')
    substitutions.HTTP_PROXY_PASSWORD_ONLY = endpoints.HTTP_PROXY.replace('http://', 'http://:synthetic-only@')
    const expand = value => value.replace(/@([A-Z_2]+)@/g, (_, key) => substitutions[key])
    const unexpand = value => {
      if (typeof value === 'string') { for (const [key, replacement] of Object.entries(substitutions).sort((a, b) => b[1].length - a[1].length)) value = value.split(replacement).join(`@${key}@`); return value }
      if (Array.isArray(value)) return value.map(unexpand)
      if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, unexpand(item)]))
      return value
    }
    const expanded = cases.map(item => {
      const expandedSource = expand(decodeURIComponent(item.source))
      const target = new URL(expandedSource).searchParams.get('to')
      return { ...item, expandedSource, expandedEnv: Object.fromEntries(Object.entries(item.env).map(([key, value]) => [key, expand(value)])), selectionUrls: target ? [expandedSource, target] : [expandedSource] }
    })
    const outputs = []
    const worker = new Worker(fileURLToPath(import.meta.url), { workerData: { cases: expanded }, env: { SYSTEMROOT: process.env.SYSTEMROOT || 'C:\\Windows' } })
    try {
      await new Promise((resolve, reject) => {
        worker.on('error', reject)
        worker.on('message', message => {
          if (message.type === 'start') { records.splice(0); worker.postMessage('ready') }
          if (message.type === 'result') { const item = cases.find(item => item.name === message.name); outputs.push({ ...item, ...unexpand(message.result), candidates: unexpand(message.candidates), records: unexpand(records.splice(0)) }); worker.postMessage('next') }
          if (message.type === 'done') resolve()
        })
      })
      const result = { sdkVersion: require('../../../../node_modules/@larksuiteoapi/node-sdk/package.json').version, axiosVersion: require('../../../../node_modules/axios/package.json').version, proxyFromEnvVersion: require('../../../../node_modules/proxy-from-env/package.json').version, nodeVersion: process.version, cases: outputs }
      fs.writeFileSync(process.argv[2], JSON.stringify(result, null, 2) + '\n')
      console.log(JSON.stringify({ sdkVersion: result.sdkVersion, axiosVersion: result.axiosVersion, proxyFromEnvVersion: result.proxyFromEnvVersion, nodeVersion: result.nodeVersion, cases: outputs.map(({ name, error, records }) => ({ name, error, routes: records.map(record => record.server + ':' + record.method) })) }))
    } finally { await worker.terminate(); await shutdown() }
  }
}
