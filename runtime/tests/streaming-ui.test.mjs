import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createStreamingTextScheduler,
  createToolUpdateScheduler,
  createTypewriterDisplay,
} from '../../src/lib/streaming-ui.ts'

test('streaming text scheduler coalesces rapid updates into one flush', async () => {
  const frames = []
  const scheduler = createStreamingTextScheduler(
    (text, activityAt) => frames.push({ text, activityAt }),
    { intervalMs: 20 },
  )
  scheduler.push('a', 't1')
  scheduler.push('ab', 't2')
  scheduler.push('abc', 't3')
  assert.deepEqual(frames, [])
  await new Promise((resolve) => setTimeout(resolve, 35))
  assert.deepEqual(frames, [{ text: 'abc', activityAt: 't3' }])
  scheduler.push('abcd', 't4')
  scheduler.flush()
  assert.deepEqual(frames.at(-1), { text: 'abcd', activityAt: 't4' })
  scheduler.cancel()
})

test('streaming scheduler defers hidden-page updates until visibility returns', async () => {
  const originalDocument = Object.getOwnPropertyDescriptor(globalThis, 'document')
  const page = new EventTarget()
  page.visibilityState = 'hidden'
  Object.defineProperty(globalThis, 'document', { configurable: true, value: page })
  const frames = []
  const scheduler = createStreamingTextScheduler((text) => frames.push(text), { intervalMs: 10 })

  try {
    scheduler.push('first')
    scheduler.push('latest')
    await new Promise((resolve) => setTimeout(resolve, 20))
    assert.deepEqual(frames, [])

    page.visibilityState = 'visible'
    page.dispatchEvent(new Event('visibilitychange'))
    await new Promise((resolve) => setTimeout(resolve, 20))
    assert.deepEqual(frames, ['latest'])
  } finally {
    scheduler.cancel()
    if (originalDocument) Object.defineProperty(globalThis, 'document', originalDocument)
    else delete globalThis.document
  }
})

test('tool update scheduler merges patches by tool id', async () => {
  const frames = []
  const scheduler = createToolUpdateScheduler(
    (batch, activityAt) => frames.push({ batch: Object.fromEntries(batch), activityAt }),
    { intervalMs: 20 },
  )
  scheduler.push('tool-1', { message: 'a' }, 't1')
  scheduler.push('tool-1', { message: 'ab', agent: { status: 'running' } }, 't2')
  scheduler.push('tool-2', { message: 'x' }, 't3')
  await new Promise((resolve) => setTimeout(resolve, 35))
  assert.equal(frames.length, 1)
  assert.deepEqual(frames[0].batch['tool-1'], { message: 'ab', agent: { status: 'running' } })
  assert.deepEqual(frames[0].batch['tool-2'], { message: 'x' })
  assert.equal(frames[0].activityAt, 't3')
  scheduler.cancel()
})

test('typewriter reveals gradually and snaps on flush', async () => {
  const frames = []
  const typewriter = createTypewriterDisplay((text) => frames.push(text), {
    minCharsPerSecond: 20,
    maxCharsPerSecond: 200,
    catchUpRemaining: 40,
    snapRemaining: 200,
  })
  typewriter.setTarget('hello world')
  await new Promise((resolve) => setTimeout(resolve, 80))
  assert.ok(frames.length >= 1)
  assert.ok(frames.at(-1).length <= 'hello world'.length)
  assert.ok(frames.at(-1).length > 0)
  typewriter.setTarget('hello world!!!')
  typewriter.flush()
  assert.equal(frames.at(-1), 'hello world!!!')
  typewriter.cancel()
})

test('typewriter pauses while hidden and catches up when visible', async () => {
  const originalDocument = Object.getOwnPropertyDescriptor(globalThis, 'document')
  const page = new EventTarget()
  page.visibilityState = 'hidden'
  Object.defineProperty(globalThis, 'document', { configurable: true, value: page })
  const frames = []
  const typewriter = createTypewriterDisplay((text) => frames.push(text))

  try {
    typewriter.setTarget('hidden response')
    await new Promise((resolve) => setTimeout(resolve, 45))
    assert.deepEqual(frames, [])

    page.visibilityState = 'visible'
    page.dispatchEvent(new Event('visibilitychange'))
    await new Promise((resolve) => setTimeout(resolve, 45))
    assert.ok(frames.at(-1).length > 0)
  } finally {
    typewriter.cancel()
    if (originalDocument) Object.defineProperty(globalThis, 'document', originalDocument)
    else delete globalThis.document
  }
})

test('typewriter snaps large backlogs in one paint', async () => {
  const frames = []
  const typewriter = createTypewriterDisplay((text) => frames.push(text), { snapRemaining: 50 })
  typewriter.setTarget('x'.repeat(120))
  await new Promise((resolve) => setTimeout(resolve, 40))
  assert.equal(frames.at(-1), 'x'.repeat(120))
  typewriter.cancel()
})
