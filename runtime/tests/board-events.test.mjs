import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { createServer, request } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler, requiredRuntimeFeature } from '../http/api-handler.mjs'
import { sessionRuntimeRoutes } from '../http/routes/sessions-runtime.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { resolveRuntimeCapabilities } from '../runtime-capabilities.mjs'
import { BoardEventHub, BoardEventsLimitError } from '../services/board-event-hub.mjs'

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

async function waitFor(check, timeoutMs = 3000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const value = check()
    if (value) return value
    await sleep(5)
  }
  assert.fail('Timed out waiting for board events')
}

// 可控的数据源：state 里有的会话上看板，没有的返回 null（不上看板 / 已删除）。
function fakeSource() {
  const state = new Map()
  const calls = []
  return {
    state,
    calls,
    set(id, value) {
      state.set(id, value)
    },
    async listItems(ids) {
      calls.push(ids ? [...ids] : null)
      const result = new Map()
      for (const id of ids || state.keys()) {
        const value = state.get(id)
        result.set(
          id,
          value
            ? { id, session: { id, name: value.name }, card: { boardStatus: value.lane } }
            : null,
        )
      }
      return result
    },
  }
}

function recorder() {
  const frames = []
  let closed = 0
  return {
    frames,
    get closed() {
      return closed
    },
    send(event, data) {
      frames.push({ event, data: JSON.parse(JSON.stringify(data)) })
      return true
    },
    close() {
      closed += 1
    },
    events(name) {
      return frames.filter((frame) => frame.event === name)
    },
  }
}

function hubFor(source, options = {}) {
  return new BoardEventHub({
    listItems: (ids) => source.listItems(ids),
    minIntervalMs: 40,
    coalesceMs: 5,
    minFlushGapMs: 10,
    heartbeatMs: 60_000,
    ...options,
  })
}

test('a new connection gets a snapshot, then per-session card deltas and removals', async (t) => {
  const source = fakeSource()
  source.set('a', { name: 'A', lane: 'running' })
  source.set('b', { name: 'B', lane: 'done' })
  const hub = hubFor(source)
  t.after(() => hub.dispose())
  const client = recorder()
  const connection = hub.connect({ send: client.send, close: client.close })
  await connection.ready

  const [snapshot] = client.events('snapshot')
  assert.equal(typeof snapshot.data.revision, 'number')
  assert.deepEqual(
    snapshot.data.items.map((item) => [item.id, item.card.boardStatus]),
    [
      ['a', 'running'],
      ['b', 'done'],
    ],
  )

  source.set('a', { name: 'A', lane: 'attention' })
  hub.touch('a')
  const [delta] = await waitFor(
    () => client.events('session_card').length && client.events('session_card'),
  )
  assert.equal(delta.data.id, 'a')
  assert.ok(delta.data.revision > snapshot.data.revision)
  assert.equal(delta.data.card.boardStatus, 'attention')
  assert.deepEqual(delta.data.session, { id: 'a', name: 'A' })

  source.state.delete('b')
  hub.touch('b')
  await waitFor(() => client.events('session_card').length === 2)
  assert.deepEqual(client.events('session_card')[1].data, {
    revision: delta.data.revision + 1,
    id: 'b',
    removed: true,
  })
  // 已知不在看板上的会话再次变化，不重复发 removed。
  hub.touch('b')
  await sleep(80)
  assert.equal(client.events('session_card').length, 2)

  hub.componentChanged('weather-widget')
  await waitFor(() => client.events('component_changed').length)
  assert.equal(client.events('component_changed')[0].data.id, 'weather-widget')
  // 旧签名只带 id 与看板修订号，不多出字段。
  assert.deepEqual(Object.keys(client.events('component_changed')[0].data).sort(), [
    'id',
    'revision',
  ])

  // A7 只做加法：rev（内容哈希）与 reason；非法取值丢弃而不是原样转发。
  hub.componentChanged('notes', { rev: 'a1b2c3d4e5f6a7b8c9d0e1f2', reason: 'files' })
  hub.componentChanged('notes', { rev: 'not-a-hash', reason: 'exploded' })
  hub.componentChanged('notes', { rev: null, reason: 'deleted' })
  await waitFor(() => client.events('component_changed').length === 4)
  const [, files, invalid, deleted] = client.events('component_changed')
  assert.equal(files.data.rev, 'a1b2c3d4e5f6a7b8c9d0e1f2')
  assert.equal(files.data.reason, 'files')
  assert.ok(files.data.revision > client.events('component_changed')[0].data.revision)
  assert.deepEqual(Object.keys(invalid.data).sort(), ['id', 'revision'])
  assert.deepEqual(
    { ...deleted.data, revision: 0 },
    { id: 'notes', revision: 0, reason: 'deleted' },
  )

  connection.detach()
  assert.deepEqual(hub.stats().connections, 0)
  assert.equal(hub.stats().timers, false)
})

test('card deltas are throttled per session to at most one per interval', async (t) => {
  const source = fakeSource()
  source.set('busy', { name: 'Busy', lane: 'running' })
  source.set('other', { name: 'Other', lane: 'running' })
  const hub = hubFor(source, { minIntervalMs: 50 })
  t.after(() => hub.dispose())
  const client = recorder()
  const times = []
  const connection = hub.connect({
    send(event, data) {
      if (event === 'session_card' && data.id === 'busy') times.push(Date.now())
      return client.send(event, data)
    },
    close: client.close,
  })
  await connection.ready
  const started = Date.now()
  while (Date.now() - started < 260) {
    hub.touch('busy')
    await sleep(2)
  }
  hub.touch('other')
  await sleep(120)
  // 260ms 内连续变化：每 50ms 最多一帧，且最后一次变化一定会被送达。
  assert.ok(times.length >= 3, `expected merged deltas, got ${times.length}`)
  assert.ok(times.length <= 8, `expected throttled deltas, got ${times.length}`)
  for (let index = 1; index < times.length; index += 1)
    assert.ok(times[index] - times[index - 1] >= 45, 'deltas for one session stay spaced out')
  // 一个会话被节流不影响另一个会话的增量。
  assert.ok(client.events('session_card').some((frame) => frame.data.id === 'other'))
  connection.detach()
})

test('reconnecting with after replays missed changes, merged per session', async (t) => {
  const source = fakeSource()
  source.set('a', { name: 'A', lane: 'running' })
  source.set('b', { name: 'B', lane: 'running' })
  const hub = hubFor(source)
  t.after(() => hub.dispose())
  const first = recorder()
  const connection = hub.connect({ send: first.send, close: first.close })
  await connection.ready
  const seen = first.events('snapshot')[0].data.revision
  connection.detach()
  assert.equal(hub.stats().timers, false)

  // 断开期间的变化只记脏标记，不计算、不跑定时器。
  source.set('a', { name: 'A2', lane: 'done' })
  hub.touch('a')
  hub.touch('a')
  source.set('c', { name: 'C', lane: 'attention' })
  hub.touch('c')
  assert.equal(hub.stats().timers, false)
  assert.equal(hub.stats().dirty, 2)

  const second = recorder()
  const resumed = hub.connect({ after: seen, send: second.send, close: second.close })
  await resumed.ready
  assert.equal(second.events('snapshot').length, 0)
  assert.equal(second.events('resync_required').length, 0)
  const replayed = second
    .events('session_card')
    .map((frame) => [frame.data.id, frame.data.card.boardStatus])
  assert.deepEqual(replayed, [
    ['a', 'done'],
    ['c', 'attention'],
  ])
  const heartbeat = second.events('heartbeat').at(-1)
  assert.equal(heartbeat.data.revision, hub.revision)

  // 带最新修订号重连：没有遗漏，也就没有补发。
  resumed.detach()
  const third = recorder()
  const upToDate = hub.connect({ after: hub.revision, send: third.send, close: third.close })
  await upToDate.ready
  assert.equal(third.events('session_card').length, 0)
  assert.equal(third.events('snapshot').length, 0)
  upToDate.detach()
})

test('gaps, unknown revisions and idle overflow force resync_required before a snapshot', async (t) => {
  const source = fakeSource()
  for (const id of ['a', 'b', 'c', 'd']) source.set(id, { name: id, lane: 'running' })
  const hub = hubFor(source, { maxBufferedEvents: 2, maxDirty: 3 })
  t.after(() => hub.dispose())
  const first = recorder()
  const connection = hub.connect({ send: first.send, close: first.close })
  await connection.ready
  const seen = first.events('snapshot')[0].data.revision
  for (const id of ['a', 'b', 'c']) hub.touch(id)
  await waitFor(() => first.events('session_card').length === 3)
  connection.detach()

  // 缓冲只留 2 帧，第一帧已被挤出：after 落在缺口里。
  const gap = recorder()
  const afterGap = hub.connect({ after: seen, send: gap.send, close: gap.close })
  await afterGap.ready
  assert.deepEqual(
    gap.frames.map((frame) => frame.event),
    ['resync_required', 'snapshot'],
  )
  assert.equal(gap.frames[0].data.reason, 'gap')
  afterGap.detach()

  // 来自未来（例如 Runtime 重启前）的修订号同样要求重新同步。
  const future = recorder()
  const fromFuture = hub.connect({
    after: hub.revision + 1000,
    send: future.send,
    close: future.close,
  })
  await fromFuture.ready
  assert.equal(future.frames[0].event, 'resync_required')
  assert.equal(future.frames[0].data.reason, 'unknown_revision')
  assert.equal(future.frames[1].event, 'snapshot')
  fromFuture.detach()

  // 空闲期脏标记溢出：记不住变了哪些会话，带旧修订号的重连必须重新同步。
  const before = hub.revision
  for (const id of ['a', 'b', 'c', 'd', 'e']) hub.touch(id)
  const overflow = recorder()
  const afterOverflow = hub.connect({ after: before, send: overflow.send, close: overflow.close })
  await afterOverflow.ready
  assert.equal(overflow.frames[0].event, 'resync_required')
  afterOverflow.detach()
})

test('connections are capped, dead writers are dropped and dispose closes everyone', async () => {
  const source = fakeSource()
  source.set('a', { name: 'A', lane: 'running' })
  const hub = hubFor(source, { maxConnections: 2, heartbeatMs: 20 })
  const healthy = recorder()
  const first = hub.connect({ send: healthy.send, close: healthy.close })
  let failures = 0
  const broken = hub.connect({
    send() {
      failures += 1
      throw new Error('socket gone')
    },
    close() {},
  })
  assert.throws(() => hub.connect({ send: () => true, close() {} }), BoardEventsLimitError)
  await Promise.all([first.ready, broken.ready])
  // 写出抛错的连接被移除，名额释放给新连接。
  assert.equal(failures, 1)
  assert.equal(hub.stats().connections, 1)
  const late = recorder()
  const third = hub.connect({ send: late.send, close: late.close })
  await third.ready
  await waitFor(() => healthy.events('heartbeat').length >= 1)
  hub.dispose()
  assert.equal(healthy.closed, 1)
  assert.equal(late.closed, 1)
  assert.deepEqual(hub.stats(), {
    connections: 0,
    buffered: 0,
    bufferedBytes: 0,
    dirty: 0,
    revision: hub.revision,
    timers: false,
  })
  assert.throws(() => hub.connect({ send: () => true, close() {} }), BoardEventsLimitError)
})

function readSse(url, { onFrame, signal }) {
  return new Promise((resolve, reject) => {
    const req = request(url, { signal }, (res) => {
      let buffer = ''
      res.setEncoding('utf8')
      res.on('data', (chunk) => {
        buffer += chunk
        let index
        while ((index = buffer.indexOf('\n\n')) >= 0) {
          const block = buffer.slice(0, index)
          buffer = buffer.slice(index + 2)
          const event = /^event: (.+)$/m.exec(block)?.[1]
          const data = /^data: (.+)$/m.exec(block)?.[1]
          if (event && data) onFrame({ event, data: JSON.parse(data), raw: data })
        }
      })
      res.on('end', () => resolve(res))
      res.on('error', () => resolve(res))
      onFrame({ event: '__response', status: res.statusCode, headers: res.headers })
    })
    req.on('error', (error) => (error.name === 'AbortError' ? resolve(null) : reject(error)))
    req.end()
  })
}

test('the SSE route streams runtime changes and cleans up when the client disconnects', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-board-events-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = { getGlobalSettings: () => ({}) }
  t.after(() => runtime.dispose().catch(() => {}))
  const created = await runtime.createSession('事件流会话', directory)
  runtime.sessionMeta[created.id] = {
    ...runtime.sessionMeta[created.id],
    organization: {
      pinned: false,
      archived: false,
      unread: true,
      failed: false,
      lastCompletedAt: new Date().toISOString(),
    },
  }
  const handleApi = createApiHandler(runtime)
  const server = createServer((req, res) => {
    void handleApi(req, res, new URL(req.url, 'http://127.0.0.1'))
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  t.after(() => new Promise((resolve) => server.close(resolve)))
  const base = `http://127.0.0.1:${server.address().port}`

  const frames = []
  const controller = new AbortController()
  const stream = readSse(`${base}/api/board/events`, {
    signal: controller.signal,
    onFrame: (frame) => frames.push(frame),
  })
  const head = await waitFor(() => frames.find((frame) => frame.event === '__response'))
  assert.equal(head.status, 200)
  assert.match(head.headers['content-type'], /text\/event-stream/)
  const snapshot = await waitFor(() => frames.find((frame) => frame.event === 'snapshot'))
  assert.deepEqual(
    snapshot.data.items.map((item) => [item.id, item.card.boardStatus]),
    [[created.id, 'done']],
  )
  assert.equal(runtime.boardEvents.stats().connections, 1)

  // 组织状态变化经投影失效收口推送：置顶后收到新卡片，归档后收到 removed。
  const organizationRoute = sessionRuntimeRoutes.find(
    (route) => route.method === 'PATCH' && route.path === '/api/sessions/:sessionId/organization',
  )
  const patch = (input) =>
    organizationRoute.handler({
      runtime,
      params: { sessionId: created.id },
      body: async () => input,
      json: () => {},
    })
  await patch({ pinned: true })
  const pinned = await waitFor(() =>
    frames.find((frame) => frame.event === 'session_card' && frame.data.session?.pinned),
  )
  assert.equal(pinned.data.id, created.id)
  assert.ok(pinned.data.revision > snapshot.data.revision)
  await patch({ archived: true })
  const removed = await waitFor(() =>
    frames.find((frame) => frame.event === 'session_card' && frame.data.removed),
  )
  assert.equal(removed.data.id, created.id)

  // 带 after 重连：只补发断开期间的变化。
  controller.abort()
  await stream
  await waitFor(() => runtime.boardEvents.stats().connections === 0)
  assert.equal(runtime.boardEvents.stats().timers, false)
  await patch({ archived: false })
  const resumedFrames = []
  const resumedController = new AbortController()
  const resumed = readSse(`${base}/api/board/events?after=${removed.data.revision}`, {
    signal: resumedController.signal,
    onFrame: (frame) => resumedFrames.push(frame),
  })
  const restored = await waitFor(() =>
    resumedFrames.find((frame) => frame.event === 'session_card' && frame.data.card),
  )
  assert.equal(restored.data.id, created.id)
  assert.equal(
    resumedFrames.some((frame) => frame.event === 'snapshot'),
    false,
  )
  resumedController.abort()
  await resumed
  await waitFor(() => runtime.boardEvents.stats().connections === 0)

  // 非法 after 按没有 after 处理：直接发快照。
  const junkFrames = []
  const junkController = new AbortController()
  const junk = readSse(`${base}/api/board/events?after=abc`, {
    signal: junkController.signal,
    onFrame: (frame) => junkFrames.push(frame),
  })
  await waitFor(() => junkFrames.find((frame) => frame.event === 'snapshot'))
  junkController.abort()
  await junk
  await waitFor(() => runtime.boardEvents.stats().connections === 0)
})

test('board capabilities are declared on every profile and gate the events route', async () => {
  for (const profile of ['desktop', 'mobile-embedded', 'mobile-store']) {
    const capabilities = await resolveRuntimeCapabilities({
      environment: { PISPER_RUNTIME_PROFILE: profile },
      moduleSupport: { childProcess: false, workerThreads: false, sqlite: false, wasm: false },
    })
    assert.equal(capabilities.features['board.view'], true, profile)
    assert.equal(capabilities.features['board.events'], true, profile)
  }
  assert.equal(requiredRuntimeFeature('/api/board/events'), 'board.events')
  assert.equal(requiredRuntimeFeature('/api/sessions'), '')
})

test('a failing data source closes the connection instead of leaving it half-open', async () => {
  let fail = true
  const errors = []
  const hub = new BoardEventHub({
    listItems: async () => {
      if (fail) throw new Error('catalog unavailable')
      return new Map([['a', { id: 'a', session: { id: 'a' }, card: { boardStatus: 'done' } }]])
    },
    minIntervalMs: 10,
    coalesceMs: 1,
    minFlushGapMs: 1,
    onError: (error) => errors.push(error.message),
  })
  const client = recorder()
  const failed = hub.connect({ send: client.send, close: client.close })
  await failed.ready
  assert.equal(client.closed, 1)
  assert.equal(hub.stats().connections, 0)
  assert.equal(hub.stats().timers, false)
  assert.deepEqual(errors, ['catalog unavailable'])

  // 数据源恢复后，在线连接遇到刷新失败会收到 resync_required，而不是悄悄丢掉变化。
  fail = false
  const online = recorder()
  const connection = hub.connect({ send: online.send, close: online.close })
  await connection.ready
  assert.equal(online.events('snapshot').length, 1)
  fail = true
  hub.touch('a')
  await waitFor(() => online.events('resync_required').length)
  assert.equal(online.events('resync_required')[0].data.reason, 'flush_failed')
  connection.detach()
  hub.dispose()
})
