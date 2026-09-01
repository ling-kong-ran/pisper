// Run 注册表与可重挂 SSE 的测试：游标分配、环形缓冲溢出、重挂补发、
// 终态保留期，以及 chat 路由的端到端 run 帧契约。
import assert from 'node:assert/strict'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { RunNotResumableError, RunRegistry } from '../services/run-registry.mjs'

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

test('游标单调递增；缓冲溢出时记录 droppedUpTo 并标记缺口', () => {
  const registry = new RunRegistry({ maxEvents: 3, maxBytes: 1024 * 1024 })
  const run = registry.begin({ kind: 'chat', sessionId: 's1' })
  for (let index = 0; index < 5; index += 1) {
    assert.equal(registry.record(run, 'text_delta', { index }), index + 1)
  }
  assert.equal(run.droppedUpTo, 2)

  const { gap, replay } = registry.prepareAttach(run.id, 0)
  assert.equal(gap, true)
  assert.deepEqual(
    replay.map((entry) => entry.cursor),
    [3, 4, 5],
  )
  // after 超过缺口线则无缺口。
  assert.equal(registry.prepareAttach(run.id, 3).gap, false)
})

test('实时订阅者收到后续帧；关闭后收到 onEnd 并在保留期后清理', async () => {
  const registry = new RunRegistry({ retentionMs: 30 })
  const run = registry.begin({})
  registry.record(run, 'snapshot', { step: 0 })

  const received = []
  let ended = false
  const sink = {
    onEvent: (cursor, event) => received.push([cursor, event]),
    onEnd: () => {
      ended = true
    },
  }
  const { replay } = registry.prepareAttach(run.id, 0)
  assert.equal(replay.length, 1)
  assert.equal(registry.subscribe(run, sink), true)

  registry.record(run, 'text_delta', { step: 1 })
  assert.deepEqual(received, [[2, 'text_delta']])

  registry.close(run)
  assert.equal(ended, true)
  // 已关闭的 run 仍可重放（终态保留期）。
  assert.equal(registry.prepareAttach(run.id, 0).replay.length, 2)
  assert.equal(registry.subscribe(run, sink), false)

  await sleep(60)
  assert.throws(
    () => registry.prepareAttach(run.id, 0),
    (error) => {
      assert.ok(error instanceof RunNotResumableError)
      assert.equal(error.code, 'run_not_resumable')
      return true
    },
  )
})

// ── HTTP 层集成 ──────────────────────────────────────────────────────

function request(method, body) {
  return {
    method,
    async *[Symbol.asyncIterator]() {
      if (body !== undefined) yield Buffer.from(JSON.stringify(body))
    },
  }
}

function response() {
  return {
    status: 0,
    body: '',
    destroyed: false,
    writableEnded: false,
    handlers: new Map(),
    writeHead(status) {
      this.status = status
    },
    flushHeaders() {},
    write(chunk = '') {
      this.body += chunk
    },
    end(chunk = '') {
      this.body += chunk
      this.writableEnded = true
    },
    on(event, callback) {
      this.handlers.set(event, callback)
    },
  }
}

// 解析 SSE 文本为帧数组：{ id, event, data }。
function parseSseFrames(text) {
  const frames = []
  for (const block of text.split('\n\n')) {
    if (!block.trim()) continue
    const frame = { id: null, event: null, data: null }
    for (const line of block.split('\n')) {
      if (line.startsWith('id: ')) frame.id = Number(line.slice(4))
      else if (line.startsWith('event: ')) frame.event = line.slice(7)
      else if (line.startsWith('data: ')) frame.data = JSON.parse(line.slice(6))
    }
    frames.push(frame)
  }
  return frames
}

test('chat 首帧为 run，后续帧带单调游标；终态后可按游标重挂补发', async () => {
  const registry = new RunRegistry()
  const runtime = {
    async streamPrompt({ send }) {
      send('snapshot', { text: 'working' })
      send('text_delta', { delta: '你好' })
      send('done', { text: '完成' })
    },
  }
  const handler = createApiHandler(runtime, { runs: registry })

  const output = response()
  await handler(
    request('POST', { sessionId: 's1', message: '你好' }),
    output,
    new URL('http://localhost/api/chat'),
  )
  const frames = parseSseFrames(output.body)
  assert.equal(frames[0].event, 'run')
  assert.equal(frames[0].data.cursor, 0)
  assert.equal(frames[0].data.kind, 'chat')
  const runId = frames[0].data.runId
  assert.deepEqual(
    frames.slice(1).map((frame) => [frame.id, frame.event]),
    [
      [1, 'snapshot'],
      [2, 'text_delta'],
      [3, 'done'],
    ],
  )

  // 重挂：after=1 只补发游标 2、3 的帧。
  const replayOutput = response()
  await handler(
    request('GET'),
    replayOutput,
    new URL(`http://localhost/api/runs/${runId}/events?after=1`),
  )
  const replayFrames = parseSseFrames(replayOutput.body)
  assert.equal(replayFrames[0].event, 'run')
  assert.equal(replayFrames[0].data.resumed, true)
  assert.deepEqual(
    replayFrames.slice(1).map((frame) => [frame.id, frame.event]),
    [
      [2, 'text_delta'],
      [3, 'done'],
    ],
  )
})

test('上游中断时 chat SSE 发出终态错误并关闭可重挂 run', async () => {
  const registry = new RunRegistry()
  const runtime = {
    async streamPrompt({ send }) {
      send('text_delta', { delta: '部分响应' })
      throw new Error('OpenAI Responses stream ended before a terminal response event')
    },
  }
  const handler = createApiHandler(runtime, { runs: registry })
  const output = response()

  await handler(
    request('POST', { sessionId: 's1', message: '继续' }),
    output,
    new URL('http://localhost/api/chat'),
  )

  const frames = parseSseFrames(output.body)
  const runId = frames[0].data.runId
  assert.deepEqual(
    frames.slice(1).map((frame) => frame.event),
    ['text_delta', 'error'],
  )
  assert.match(frames.at(-1).data.message, /terminal response event/)
  assert.equal(registry.get(runId).closed, true)

  const replayOutput = response()
  await handler(
    request('GET'),
    replayOutput,
    new URL(`http://localhost/api/runs/${runId}/events?after=0`),
  )
  assert.deepEqual(
    parseSseFrames(replayOutput.body)
      .slice(1)
      .map((frame) => frame.event),
    ['text_delta', 'error'],
  )
})

test('chat handler without an explicit terminal emits a resumable error snapshot', async () => {
  const registry = new RunRegistry()
  const handler = createApiHandler(
    {
      async streamPrompt() {},
    },
    { runs: registry },
  )
  const output = response()

  await handler(
    request('POST', { sessionId: 's1', message: '静默结束' }),
    output,
    new URL('http://localhost/api/chat'),
  )

  const frames = parseSseFrames(output.body)
  const runId = frames[0].data.runId
  assert.equal(frames.at(-1).event, 'error')
  assert.match(frames.at(-1).data.message, /发送终态前结束/)
  assert.equal(registry.get(runId).closed, true)
})

test('重挂进行中的 run：先补发缓存，再接收实时帧直到关闭', async () => {
  const registry = new RunRegistry()
  let releaseStream
  const runtime = {
    async streamPrompt({ send }) {
      send('snapshot', { text: 'working' })
      await new Promise((resolve) => {
        releaseStream = () => {
          send('done', { text: '完成' })
          resolve()
        }
      })
    },
  }
  const handler = createApiHandler(runtime, { runs: registry })

  const liveOutput = response()
  const pending = handler(
    request('POST', { sessionId: 's1', message: '开始' }),
    liveOutput,
    new URL('http://localhost/api/chat'),
  )
  // 等首帧写出后拿到 runId。
  await sleep(10)
  const runId = parseSseFrames(liveOutput.body)[0].data.runId

  const replayOutput = response()
  const reattached = handler(
    request('GET'),
    replayOutput,
    new URL(`http://localhost/api/runs/${runId}/events?after=0`),
  )
  await sleep(10)
  releaseStream()
  await Promise.all([pending, reattached])

  const frames = parseSseFrames(replayOutput.body)
  assert.deepEqual(
    frames.map((frame) => frame.event),
    ['run', 'snapshot', 'done'],
  )
  assert.equal(replayOutput.writableEnded, true)
})

test('未知 run 重挂返回 409 与机读错误码', async () => {
  const handler = createApiHandler({}, { runs: new RunRegistry() })
  const output = response()
  await handler(
    request('GET'),
    output,
    new URL('http://localhost/api/runs/run_missing/events?after=0'),
  )
  assert.equal(output.status, 409)
  assert.equal(JSON.parse(output.body).code, 'run_not_resumable')
})
