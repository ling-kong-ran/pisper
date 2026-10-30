import assert from 'node:assert/strict'
import { mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler, requiredRuntimeFeature } from '../http/api-handler.mjs'
import { SessionQueueError, SessionStartQueue } from '../services/session-start-queue.mjs'
import { projectSessionCard } from '../services/session-card-summary.mjs'

async function settle() {
  for (let index = 0; index < 5; index += 1) await new Promise((resolve) => setImmediate(resolve))
}

function harness({ path = '', running = [], sessions = ['a', 'b', 'c', 'd'] } = {}) {
  const runningSet = new Set(running)
  const started = []
  const changed = []
  const queue = new SessionStartQueue({
    path,
    startSession: (entry) => {
      started.push(entry)
      runningSet.add(entry.sessionId)
    },
    isRunning: (id) => runningSet.has(id),
    sessionExists: (id) => sessions.includes(id),
    onChange: (id) => changed.push(id),
  })
  return { queue, started, changed, running: runningSet }
}

test('a session queued after a running one starts when that run finishes', async () => {
  const { queue, started, changed, running } = harness({ running: ['a'] })
  const result = await queue.enqueue({
    sessionId: 'b',
    startAfter: 'a',
    message: '  update the README ',
    runMode: 'goal',
  })
  assert.equal(result.queued, true)
  assert.equal(result.queue.position, 1)
  assert.equal(result.queue.startAfter, 'a')
  assert.deepEqual(changed, ['b'])
  assert.deepEqual(started, [])
  running.delete('a')
  assert.equal(await queue.onRunFinished('a'), 1)
  await settle()
  assert.deepEqual(started, [{ sessionId: 'b', message: 'update the README', runMode: 'goal' }])
  assert.equal(queue.summary('b'), null)
})

test('chains wait for each predecessor; positions follow creation order', async () => {
  const { queue, started, running } = harness({ running: ['a'] })
  await queue.enqueue({ sessionId: 'b', startAfter: 'a', message: 'one' })
  // c 排在 b 之后：b 还在排队（将来会结束一次），所以 c 也排队。
  const c = await queue.enqueue({ sessionId: 'c', startAfter: 'b', message: 'two' })
  assert.equal(c.queued, true)
  assert.equal(queue.summary('c').position, 2)
  running.delete('a')
  await queue.onRunFinished('a')
  await settle()
  assert.deepEqual(
    started.map((entry) => entry.sessionId),
    ['b'],
  )
  assert.equal(queue.summary('c').position, 1)
  running.delete('b')
  await queue.onRunFinished('b')
  await settle()
  assert.deepEqual(
    started.map((entry) => entry.sessionId),
    ['b', 'c'],
  )
})

test('enqueue validates input and starts immediately when the predecessor already ended', async () => {
  const { queue, started } = harness({ running: ['a'] })
  const cases = [
    [{ sessionId: 'b', startAfter: '', message: 'x' }, 'start_after_required'],
    [{ sessionId: 'b', startAfter: 'b', message: 'x' }, 'start_after_self'],
    [{ sessionId: 'b', startAfter: 'a', message: '   ' }, 'message_required'],
    [{ sessionId: 'zz', startAfter: 'a', message: 'x' }, 'session_not_found'],
    [{ sessionId: 'b', startAfter: 'zz', message: 'x' }, 'start_after_not_found'],
    [{ sessionId: 'a', startAfter: 'b', message: 'x' }, 'session_running'],
    [{ sessionId: 'b', startAfter: 'a', message: 'x'.repeat(100_001) }, 'message_too_long'],
  ]
  for (const [input, code] of cases) {
    await assert.rejects(
      queue.enqueue(input),
      (error) => error instanceof SessionQueueError && error.code === code,
      code,
    )
  }
  await queue.enqueue({ sessionId: 'b', startAfter: 'a', message: 'x' })
  await assert.rejects(queue.enqueue({ sessionId: 'b', startAfter: 'a', message: 'x' }), {
    code: 'already_queued',
  })
  // a 等 b、b 等 a：环。
  await assert.rejects(queue.enqueue({ sessionId: 'a', startAfter: 'b', message: 'x' }), {
    code: 'session_running',
  })
  const looping = harness({ running: [] })
  looping.queue.entries.set('b', {
    sessionId: 'b',
    startAfter: 'c',
    message: 'x',
    runMode: 'plan',
    createdAt: new Date().toISOString(),
  })
  await assert.rejects(looping.queue.enqueue({ sessionId: 'c', startAfter: 'b', message: 'x' }), {
    code: 'start_after_cycle',
  })
  // 前置会话 c 既没在运行也没在排队：等不到它结束，直接启动。
  const immediate = await queue.enqueue({ sessionId: 'd', startAfter: 'c', message: 'go' })
  assert.deepEqual(immediate, { queued: false, started: true })
  await settle()
  assert.deepEqual(
    started.map((entry) => entry.sessionId),
    ['d'],
  )
})

test('start now, cancel, and deleting either side of the queue', async () => {
  const { queue, started } = harness({ running: ['a'] })
  await queue.enqueue({ sessionId: 'b', startAfter: 'a', message: 'first' })
  await queue.enqueue({ sessionId: 'c', startAfter: 'a', message: 'second' })
  await queue.enqueue({ sessionId: 'd', startAfter: 'a', message: 'third' })
  assert.deepEqual(await queue.startNow('b'), { started: true })
  await settle()
  assert.deepEqual(await queue.cancel('c'), { cancelled: true, message: 'second' })
  await assert.rejects(queue.cancel('c'), { code: 'not_queued' })
  await assert.rejects(queue.startNow('c'), { code: 'not_queued' })
  // 删除前置会话 a：排在它后面的 d 视为前置已结束，照常启动。
  await queue.onSessionDeleted('a')
  await settle()
  assert.deepEqual(
    started.map((entry) => entry.sessionId),
    ['b', 'd'],
  )
  // 删除排队中的会话自己：排队作废，不启动。
  const other = harness({ running: ['a'] })
  await other.queue.enqueue({ sessionId: 'b', startAfter: 'a', message: 'x' })
  await other.queue.onSessionDeleted('b')
  await other.queue.onRunFinished('a')
  await settle()
  assert.deepEqual(other.started, [])
})

test('queue persists atomically and resumes after a restart', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'pisper-queue-'))
  try {
    const path = join(dir, 'pisper-session-queue.json')
    const first = harness({ path, running: ['a'] })
    await first.queue.enqueue({ sessionId: 'b', startAfter: 'a', message: 'after a' })
    await first.queue.enqueue({ sessionId: 'c', startAfter: 'b', message: 'after b' })
    const stored = JSON.parse(await readFile(path, 'utf8'))
    assert.equal(stored.version, 1)
    assert.deepEqual(
      stored.entries.map((entry) => [entry.sessionId, entry.startAfter]),
      [
        ['b', 'a'],
        ['c', 'b'],
      ],
    )
    // 重启：a 的那一轮随进程结束了；b 启动，c 继续等 b。
    const second = harness({ path, running: [] })
    assert.equal(await second.queue.resume(), 1)
    await settle()
    assert.deepEqual(
      second.started.map((entry) => entry.sessionId),
      ['b'],
    )
    assert.equal(second.queue.summary('c').startAfter, 'b')
    // 坏文件或坏条目不会挡住启动。
    await writeFile(
      path,
      JSON.stringify({ version: 1, entries: [{ sessionId: 'x' }, null, 'junk'] }),
    )
    const third = harness({ path })
    await third.queue.load()
    assert.deepEqual(third.queue.list(), [])
    await writeFile(path, '{not json')
    const fourth = harness({ path })
    await fourth.queue.load()
    assert.deepEqual(fourth.queue.list(), [])
    // 损坏的文件改名留底，不被覆盖。
    const kept = (await readdir(dir)).filter((name) => name.includes('.corrupt-'))
    assert.equal(kept.length, 1)
    assert.equal(await readFile(join(dir, kept[0]), 'utf8'), '{not json')
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})

test('queued sessions appear in the queued lane only with the sessionQueue capability', () => {
  const session = { id: 'b', streaming: false, modified: '2026-10-30T10:00:00.000Z' }
  const queue = { position: 1, startAfter: 'a', createdAt: '2026-10-30T10:00:00.000Z' }
  const now = Date.parse('2026-10-30T10:05:00.000Z')
  const card = projectSessionCard({ session, queue, capabilities: { sessionQueue: true }, now })
  assert.equal(card.boardStatus, 'queued')
  assert.deepEqual(card.queue, queue)
  assert.notEqual(projectSessionCard({ session, queue, now }).boardStatus, 'queued')
  const running = projectSessionCard({
    session: { ...session, streaming: true },
    queue,
    capabilities: { sessionQueue: true },
    now,
  })
  assert.equal(running.queue, undefined)
})

test('queue routes map errors to structured responses and are capability gated', async () => {
  assert.equal(requiredRuntimeFeature('/api/sessions/s/queue', 'POST'), 'sessionQueue')
  assert.equal(requiredRuntimeFeature('/api/sessions/s/queue/start', 'POST'), 'sessionQueue')
  const { queue } = harness({ running: ['a'] })
  const runtime = {
    capabilities: { features: { sessionQueue: true } },
    enqueueSessionStart: (sessionId, input) => queue.enqueue({ ...input, sessionId }),
    startQueuedSession: (sessionId) => queue.startNow(sessionId),
    cancelQueuedSession: (sessionId) => queue.cancel(sessionId),
  }
  const handler = createApiHandler(runtime)
  const call = async (method, path, payload) => {
    const request = {
      method,
      headers: { 'content-type': 'application/json' },
      async *[Symbol.asyncIterator]() {
        if (payload !== undefined) yield Buffer.from(JSON.stringify(payload))
      },
    }
    const response = {
      status: 0,
      body: '',
      writeHead(status) {
        this.status = status
      },
      setHeader() {},
      end(body) {
        this.body = body
      },
    }
    await handler(request, response, new URL(`http://localhost${path}`))
    return { status: response.status, body: JSON.parse(response.body) }
  }
  const queued = await call('POST', '/api/sessions/b/queue', { startAfter: 'a', message: 'go' })
  assert.equal(queued.status, 200)
  assert.equal(queued.body.queued, true)
  const duplicate = await call('POST', '/api/sessions/b/queue', { startAfter: 'a', message: 'go' })
  assert.equal(duplicate.status, 409)
  assert.equal(duplicate.body.code, 'already_queued')
  const missing = await call('POST', '/api/sessions/zz/queue', { startAfter: 'a', message: 'go' })
  assert.equal(missing.status, 404)
  const cancelled = await call('DELETE', '/api/sessions/b/queue')
  assert.deepEqual(cancelled.body, { cancelled: true, message: 'go' })
  const notQueued = await call('POST', '/api/sessions/b/queue/start')
  assert.equal(notQueued.status, 404)
  assert.equal(notQueued.body.code, 'not_queued')
  const gated = createApiHandler({ capabilities: { features: { sessionQueue: false } } })
  const response = {
    status: 0,
    body: '',
    writeHead(status) {
      this.status = status
    },
    setHeader() {},
    end(body) {
      this.body = body
    },
  }
  await gated(
    { method: 'DELETE', headers: {}, async *[Symbol.asyncIterator]() {} },
    response,
    new URL('http://localhost/api/sessions/b/queue'),
  )
  assert.equal(response.status, 409)
})
