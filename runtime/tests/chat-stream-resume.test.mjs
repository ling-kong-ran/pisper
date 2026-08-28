import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { streamEventsWithResume } from '../../src/lib/api.ts'
import { ApiError } from '../../src/lib/http.ts'

// 把若干 SSE 帧字符串编成一个 fetch Response；fail=true 时在末尾制造传输层中断
//（模拟连接被重置/代理提前收尾），否则干净 EOF。
function sseResponse(frames, { fail = false } = {}) {
  const encoder = new TextEncoder()
  let index = 0
  const stream = new ReadableStream({
    pull(controller) {
      if (index < frames.length) {
        controller.enqueue(encoder.encode(frames[index]))
        index += 1
      } else if (fail) {
        controller.error(new TypeError('network error'))
      } else {
        controller.close()
      }
    },
  })
  return new Response(stream, { status: 200 })
}

test('mid-stream transport failure resumes from the last cursor', async () => {
  const events = []
  const resumes = []
  await streamEventsWithResume({
    open: async () =>
      sseResponse(
        ['event: run\ndata: {"runId":"r1"}\n\n', 'id: 1\nevent: text_delta\ndata: {"d":"a"}\n\n'],
        { fail: true },
      ),
    resume: async (runId, cursor) => {
      resumes.push([runId, cursor])
      return sseResponse([
        'id: 2\nevent: text_delta\ndata: {"d":"b"}\n\n',
        'id: 3\nevent: done\ndata: {"text":"ab"}\n\n',
      ])
    },
    onEvent: (event, data) => {
      events.push([event, data])
      if (event === 'done') return false
      return undefined
    },
    baseDelayMs: 0,
  })
  // 重挂携带 runId 与已收到的游标；事件序列无缺失、无重复。
  assert.deepEqual(resumes, [['r1', 1]])
  assert.deepEqual(
    events.map(([event]) => event),
    ['run', 'text_delta', 'text_delta', 'done'],
  )
  assert.deepEqual(events[2][1], { d: 'b' })
})

test('handler stop (done) never triggers a resume', async () => {
  let resumeCalls = 0
  await streamEventsWithResume({
    open: async () =>
      sseResponse(['event: run\ndata: {"runId":"r1"}\n\n', 'id: 1\nevent: done\ndata: {}\n\n']),
    resume: async () => {
      resumeCalls += 1
      return sseResponse([])
    },
    onEvent: () => false,
    baseDelayMs: 0,
  })
  assert.equal(resumeCalls, 0)
})

test('handler errors (business error frame) propagate without resume', async () => {
  let resumeCalls = 0
  await assert.rejects(
    streamEventsWithResume({
      open: async () => sseResponse(['event: error\ndata: {"message":"boom"}\n\n'], { fail: true }),
      resume: async () => {
        resumeCalls += 1
        return sseResponse([])
      },
      onEvent: () => {
        throw new Error('handler failure')
      },
      baseDelayMs: 0,
    }),
    /handler failure/,
  )
  assert.equal(resumeCalls, 0)
})

test('409 from the resume endpoint is not retried', async () => {
  let resumeCalls = 0
  await assert.rejects(
    streamEventsWithResume({
      open: async () => sseResponse(['event: run\ndata: {"runId":"r1"}\n\n'], { fail: true }),
      resume: async () => {
        resumeCalls += 1
        return new Response(JSON.stringify({ error: 'gone', code: 'run_not_resumable' }), {
          status: 409,
          headers: { 'Content-Type': 'application/json' },
        })
      },
      onEvent: () => {},
      baseDelayMs: 0,
    }),
    (error) => error instanceof ApiError && error.status === 409,
  )
  assert.equal(resumeCalls, 1)
})

test('resume retries transport failures and gives up after maxAttempts', async () => {
  let resumeCalls = 0
  await assert.rejects(
    streamEventsWithResume({
      open: async () => sseResponse(['event: run\ndata: {"runId":"r1"}\n\n'], { fail: true }),
      resume: async () => {
        resumeCalls += 1
        throw new TypeError('network error')
      },
      onEvent: () => {},
      baseDelayMs: 0,
      maxAttempts: 3,
    }),
    /network error/,
  )
  assert.equal(resumeCalls, 3)
})

test('progress on a resume resets the failure budget', async () => {
  let resumeCalls = 0
  await streamEventsWithResume({
    open: async () => sseResponse(['event: run\ndata: {"runId":"r1"}\n\n'], { fail: true }),
    resume: async (_runId, cursor) => {
      resumeCalls += 1
      if (resumeCalls === 1) {
        // 有进展再断：游标前移，失败计数应被重置而不是累积。
        return sseResponse(['id: 5\nevent: text_delta\ndata: {"d":"x"}\n\n'], { fail: true })
      }
      assert.equal(cursor, 5)
      return sseResponse(['id: 6\nevent: done\ndata: {}\n\n'])
    },
    onEvent: (event) => (event === 'done' ? false : undefined),
    baseDelayMs: 0,
    maxAttempts: 1,
  })
  assert.equal(resumeCalls, 2)
})

test('missing run frame leaves no resumable state and throws the transport error', async () => {
  await assert.rejects(
    streamEventsWithResume({
      open: async () => sseResponse([], { fail: true }),
      resume: async () => {
        throw new Error('must not resume')
      },
      onEvent: () => {},
      baseDelayMs: 0,
    }),
    /network error/,
  )
})

test('clean EOF on resume without a terminal frame resolves quietly', async () => {
  // 终态帧被挤出缓冲的收尾场景：不抛错，交给调用方整体校准。
  await streamEventsWithResume({
    open: async () => sseResponse(['event: run\ndata: {"runId":"r1"}\n\n'], { fail: true }),
    resume: async () => sseResponse([]),
    onEvent: () => {},
    baseDelayMs: 0,
  })
})

test('resync_required hands the session over to snapshot polling', async () => {
  // 接线断言：缓冲溢出缺口时放弃流所有权并移交实时快照轮询。
  const source = await readFile('src/features/chat/use-prompt-commands.ts', 'utf8')
  assert.match(source, /event === 'resync_required'/)
  assert.match(
    source,
    /resync_required[\s\S]*?localStreamSessionsRef\.current\.delete\(sessionId\)/,
  )
  assert.match(source, /resync_required[\s\S]*?void syncLiveSession\(sessionId\)/)
})
