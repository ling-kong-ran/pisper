import assert from 'node:assert/strict'
import test from 'node:test'
import { applyTextPatch, consumeEventStream } from '../../src/lib/api.ts'

function chunkedResponse(chunks) {
  const encoder = new TextEncoder()
  return new Response(
    new ReadableStream({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk))
        controller.close()
      },
    }),
    { status: 200 },
  )
}

test('SSE client preserves event order across arbitrary chunk boundaries', async () => {
  const response = chunkedResponse([
    'event: text_patch\r\ndata: {"start":0,',
    '"text":"## 最"}\r\n\r\nevent: text_patch\r\n',
    'data: {"start":4,"text":"终建议"}\r\n\r\n',
  ])
  const events = []
  await consumeEventStream(response, (event, data) => events.push({ event, data }))
  assert.deepEqual(events, [
    { event: 'text_patch', data: { start: 0, text: '## 最' } },
    { event: 'text_patch', data: { start: 4, text: '终建议' } },
  ])
  assert.equal(
    events.reduce((text, item) => applyTextPatch(text, item.data), ''),
    '## 最终建议',
  )
})

test('SSE client recovers when a record separator is missing', async () => {
  const response = chunkedResponse([
    'event: text_patch\ndata: {"start":0,"text":"first"}\n',
    'event: text_patch\ndata: {"start":5,"text":" second"}\n\n',
  ])
  const events = []
  await consumeEventStream(response, (event, data) => events.push({ event, data }))
  assert.deepEqual(events, [
    { event: 'text_patch', data: { start: 0, text: 'first' } },
    { event: 'text_patch', data: { start: 5, text: ' second' } },
  ])
})

test('SSE client stops immediately when a terminal event is handled', async () => {
  const encoder = new TextEncoder()
  let cancelled = false
  const response = new Response(
    new ReadableStream({
      start(controller) {
        controller.enqueue(
          encoder.encode('event: done\ndata: {"finishedAt":"2026-07-23T10:00:00.000Z"}\n\n'),
        )
      },
      cancel() {
        cancelled = true
      },
    }),
    { status: 200 },
  )
  const events = []
  await consumeEventStream(response, (event, data) => {
    events.push({ event, data })
    return event === 'done' ? false : undefined
  })
  assert.deepEqual(events, [{ event: 'done', data: { finishedAt: '2026-07-23T10:00:00.000Z' } }])
  assert.equal(cancelled, true)
})

test('SSE client flushes a final record without a blank line', async () => {
  const response = chunkedResponse(['event: done\r\ndata: {"ok":true}'])
  const events = []
  await consumeEventStream(response, (event, data) => events.push({ event, data }))
  assert.deepEqual(events, [{ event: 'done', data: { ok: true } }])
})
