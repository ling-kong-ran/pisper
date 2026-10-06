// Fresh actual SDK oracle; no credentials, profile reads, or external sockets.
import fs from 'node:fs'
import http from 'node:http'
import { createRequire } from 'node:module'
const require = createRequire(import.meta.url)
const sdk = require('../../../../node_modules/@larksuiteoapi/node-sdk')
const logger = { trace() {}, debug() {}, info() {}, warn() {}, error() {} }
const options = {
  appId: 'cli_0123456789abcdef', appSecret: 'synthetic-only', logger,
  loggerLevel: sdk.LoggerLevel.error,
  safety: { chatQueue: { enabled: false } },
  policy: { dmMode: 'open', requireMention: true, respondToMentionAll: false },
}
const markdownInputs = [
  ['ordinary', '# title\n## two\n```rust\n# protected\n```', 3500],
  ['unclosed', '# title\n```rust\n# exposed\n## exposed too\n', 3500],
  ['four_fence', '# title\n````rust\n# protected\n````\n# after', 3500],
  ['shorter_close', '# title\n````rust\n# protected\n```\n# after', 3500],
  ['longer_close', '# title\n```rust\n# exposed\n````\n# after', 3500],
  ['indented', '# title\n ```rust\n# exposed\n ```\n', 3500],
  ['language_space', '# title\n``` rust extra\n# protected\n```\n', 3500],
  ['tilde', '# title\n~~~rust\n# exposed\n~~~\n', 3500],
  ['empty_fence', '# title\n```\n```\n# after', 3500],
  ['closed_heading_only', '```rust\n# protected\n```\n#### demoted', 3500],
  ['empty_heading', '# \n## \n### text\n###### text\n', 3500],
  ['crlf', '# title\r\n```rust\r\n# exposed\r\n```\r\n', 3500],
  ['line_separators', '# one\r## two\u2028### three\u2029#### four\n', 3500],
  ['literal_placeholder', '___CB_0___\n```rust\n# protected\n```\n# after', 3500],
  ['blank_lines', '# title\n\n\n\n```rust\na\n\n\n\nb\n```\n\n\n', 3500],
  ['chunk_unclosed', '# title\n```rust\n12345678901234567890\n# heading\nend\n', 24],
  ['chunk_unicode', '😀😀😀😀\n```c_1\n😀😀😀😀\n# heading\n```\nend\n', 16],
  ['chunk_fence_cr', '```rust\r\n123456789012345\n# inside\n```\r\nend', 16],
  ['oversized_line', '```rust\n' + 'x'.repeat(30) + '\nend\n```', 16],
  ['production_chunk', '# title\n```rust\n' + 'x'.repeat(3480) + '\n# inside\nend\n', 3500],
  ['fractional_near_full', '1234567890123\n# heading\nend', 17],
  ['nonascii_heading_space', '1234567890123\n#\u00a0heading\nend', 17],
]
const markdown = []
for (const [name, input, limit] of markdownInputs) {
  const channel = sdk.createLarkChannel({ ...options, outbound: { textChunkLimit: limit } })
  const posts = []
  channel.rawClient.im.v1.message.create = async ({ data }) => {
    posts.push(JSON.parse(data.content)); return { data: { message_id: 'om_synthetic' } }
  }
  await channel.sender.send('oc_synthetic', { markdown: input })
  markdown.push({ name, input, limit, posts })
  await channel.safety.dispose()
}

const channel = sdk.createLarkChannel(options)
channel.registerDispatcherHandlers()
const received = [], errors = [], acknowledgements = []
channel.on({ message: value => received.push(value), error: error => errors.push({ code: error.code, message: error.message }) })
const ws = new sdk.WSClient(options)
ws.eventDispatcher = channel.dispatcher
ws.sendMessage = frame => acknowledgements.push(JSON.parse(new TextDecoder().decode(frame.payload)))
const malformedSenders = [
  ['absent', undefined], ['null', null], ['empty', {}],
  ['sender_id_null', { sender_id: null }], ['scalar_sender', true],
  ['scalar_sender_id', { sender_id: 7 }], ['empty_sender_id', { sender_id: {} }],
]
const malformed = []
for (const [name, sender] of malformedSenders) {
  const event = { message: { message_id: 'om_' + name, chat_id: 'oc_synthetic', chat_type: 'p2p', message_type: 'text', content: '{"text":"synthetic"}', create_time: '0' } }
  if (sender !== undefined) event.sender = sender
  const messageCount = received.length, errorCount = errors.length
  await ws.handleEventData({ headers: [{ key: 'type', value: 'event' }, { key: 'message_id', value: name }, { key: 'sum', value: '1' }, { key: 'seq', value: '0' }], payload: Buffer.from(JSON.stringify({ schema: '2.0', header: { event_type: 'im.message.receive_v1' }, event })) })
  malformed.push({ name, event, error: errors[errorCount] ?? null, emitted: received.length - messageCount, ack: acknowledgements.at(-1), value: received[messageCount] ? { ...received[messageCount], peerId: received[messageCount].chatId } : null })
}
ws.close({ force: true }); await channel.safety.dispose()

// Execute the SDK's real DataCache and sweep callback under a deterministic clock.
const originalNow = Date.now, originalInterval = globalThis.setInterval, originalClear = globalThis.clearInterval
let virtualNow = 0, sweep, intervalMs
globalThis.setInterval = (callback, delay) => { sweep = callback; intervalMs = delay; return { unref() {} } }
globalThis.clearInterval = () => {}
Date.now = () => virtualNow
let virtualWs
const fragments = []
try {
  virtualWs = new sdk.WSClient(options)
  const cache = virtualWs.dataCache
  const cases = [
    [1000, 'merge', 'late', 0], [10000, 'sweep', 'late'], [11001, 'merge', 'late', 1],
    [12000, 'merge', 'expired', 0], [20000, 'sweep', 'expired'],
    [22001, 'inspect', 'expired'], [30000, 'sweep', 'expired'],
    [30000, 'merge', 'boundary', 0], [40000, 'sweep', 'boundary'], [50000, 'sweep', 'boundary'],
  ]
  for (const [at, action, id, seq] of cases) {
    virtualNow = at
    let result = null
    if (action === 'sweep') sweep()
    if (action === 'merge') result = cache.mergeData({ message_id: id, sum: 2, seq, trace_id: 'synthetic', data: Buffer.from(seq ? 'true}' : '{"ok":') })
    fragments.push({ at, action, id, seq: seq ?? null, result, cached: cache.cache.has(id) })
  }
} finally {
  virtualWs?.close({ force: true })
  Date.now = originalNow; globalThis.setInterval = originalInterval; globalThis.clearInterval = originalClear
}

const requests = []
let port
const server = http.createServer((request, response) => {
  const url = new URL(request.url, 'http://127.0.0.1')
  requests.push(url.pathname + url.search)
  const redirect = location => { response.writeHead(302, { Location: location }); response.end() }
  if (url.pathname === '/relative') return redirect('/final')
  if (url.pathname === '/absolute') return redirect(`http://127.0.0.1:${port}/final`)
  if (url.pathname === '/host-change') return redirect(`http://redirect.invalid:${port}/final`)
  if (url.pathname === '/cross-protocol') return redirect(`https://127.0.0.1:${port}/final`)
  if (url.pathname === '/loop') return redirect('/loop')
  if (url.pathname === '/chain' && Number(url.searchParams.get('n')) > 0) return redirect('/chain?n=' + (Number(url.searchParams.get('n')) - 1))
  if (url.pathname === '/no-location') { response.writeHead(302); return response.end('redirect without location') }
  if (url.pathname === '/failed') { response.writeHead(404); return response.end('absent') }
  response.end('synthetic media body')
})
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve)); port = server.address().port
const mediaChannel = sdk.createLarkChannel({ ...options, outbound: { ssrfGuard: { allowlist: ['127.0.0.1', 'localhost'] } } })
// Disable environment proxies in this oracle only; the real SDK axios transport,
// public-URL validation, pinned Agent and follow-redirects implementation run.
const originalRequest = mediaChannel.rawClient.httpInstance.request.bind(mediaChannel.rawClient.httpInstance)
mediaChannel.rawClient.httpInstance = { request: opts => originalRequest({ ...opts, proxy: false }) }
const redirects = []
try {
  for (const path of ['/relative', '/absolute', '/host-change', '/chain?n=21', '/chain?n=22', '/loop', '/no-location', '/failed', '/cross-protocol', '/dns-source']) {
    const offset = requests.length
    try {
      const body = await mediaChannel.sender.uploader.toBuffer(`http://${path === '/dns-source' ? 'localhost' : '127.0.0.1'}:${port}${path}`)
      redirects.push({ path, body: body.toString(), error: null, requests: requests.slice(offset) })
    } catch (error) {
      redirects.push({ path, body: null, error: { code: error.code, message: error.message, cause: { code: error.cause?.code, message: error.cause?.message } }, requests: requests.slice(offset) })
    }
  }
} finally {
  await mediaChannel.safety.dispose()
  await new Promise(resolve => server.close(resolve))
}
const result = { sdkVersion: require('../../../../node_modules/@larksuiteoapi/node-sdk/package.json').version, nodeVersion: process.version, markdown, malformed, fragments: { intervalMs, operations: fragments }, redirects }
fs.writeFileSync(process.argv[2], JSON.stringify(result, null, 2) + '\n')
console.log(JSON.stringify({ sdkVersion: result.sdkVersion, nodeVersion: result.nodeVersion, markdown: markdown.length, malformed: malformed.length, fragmentOperations: fragments.length, sweepIntervalMs: intervalMs, redirects: redirects.map(({ path, error }) => ({ path, error })) }))
