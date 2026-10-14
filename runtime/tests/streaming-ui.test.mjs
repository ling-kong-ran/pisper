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

function frameClock() {
  let timestamp = 0
  let nextId = 0
  const pending = new Map()
  return {
    now: () => timestamp,
    requestFrame(callback) {
      const id = nextId++
      pending.set(id, callback)
      return id
    },
    cancelFrame(id) {
      pending.delete(id)
    },
    tick(milliseconds = 1_000 / 120) {
      timestamp += milliseconds
      const callbacks = [...pending.values()]
      pending.clear()
      for (const callback of callbacks) callback(timestamp)
    },
    get pending() {
      return pending.size
    },
  }
}

function setGlobal(t, name, value) {
  const original = Object.getOwnPropertyDescriptor(globalThis, name)
  Object.defineProperty(globalThis, name, { configurable: true, value })
  t.after(() => {
    if (original) Object.defineProperty(globalThis, name, original)
    else delete globalThis[name]
  })
}

function typewriterFixture(t, options = {}) {
  const clock = frameClock()
  const frames = []
  const typewriter = createTypewriterDisplay(
    (text, activityAt) => frames.push({ text, activityAt, at: clock.now() }),
    {
      requestFrame: clock.requestFrame,
      cancelFrame: clock.cancelFrame,
      now: clock.now,
      ...options,
    },
  )
  t.after(() => typewriter.cancel())
  return { clock, frames, typewriter }
}

test('browser defaults use paired animation frames, including a zero request id', (t) => {
  const clock = frameClock()
  const cancelled = []
  setGlobal(t, 'requestAnimationFrame', function (callback) {
    assert.equal(this, globalThis)
    return clock.requestFrame(callback)
  })
  setGlobal(t, 'cancelAnimationFrame', function (id) {
    assert.equal(this, globalThis)
    cancelled.push(id)
    clock.cancelFrame(id)
  })
  const frames = []
  const typewriter = createTypewriterDisplay((text) => frames.push(text), { now: clock.now })
  t.after(() => typewriter.cancel())
  typewriter.setTarget('first')
  typewriter.setTarget('latest')
  assert.equal(clock.pending, 1)
  typewriter.flush()
  assert.deepEqual(cancelled, [0])
  assert.deepEqual(frames, ['latest'])
  typewriter.setTarget('latest text')
  typewriter.cancel()
  assert.deepEqual(cancelled, [0, 1])
  clock.tick(1_000)
  assert.deepEqual(frames, ['latest'])
})

for (const partialBrowser of [false, true]) {
  test(`timer fallback uses batch snap without a complete browser frame pair (${partialBrowser})`, (t) => {
    t.mock.timers.enable({ apis: ['setTimeout'] })
    setGlobal(
      t,
      'requestAnimationFrame',
      partialBrowser ? () => assert.fail('unpaired RAF') : undefined,
    )
    setGlobal(t, 'cancelAnimationFrame', undefined)
    let timestamp = 0
    const frames = []
    const typewriter = createTypewriterDisplay((text) => frames.push(text), {
      now: () => timestamp,
    })
    t.after(() => typewriter.cancel())
    typewriter.setTarget('x'.repeat(1_000))
    timestamp = 50
    t.mock.timers.tick(50)
    assert.deepEqual(frames, ['x'.repeat(1_000)])
  })
}

test('large default backlogs settle in one batch instead of per-character frames', (t) => {
  const { clock, frames, typewriter } = typewriterFixture(t)
  typewriter.setTarget('x'.repeat(10_000))
  clock.tick(50)
  assert.equal(frames.length, 1)
  assert.equal(frames[0].text.length, 10_000)
})

test('continuous bursts use batch snap once the backlog reaches the threshold', (t) => {
  const { clock, frames, typewriter } = typewriterFixture(t)
  const target = 'x'.repeat(600)
  typewriter.setTarget(target)
  clock.tick(50)
  assert.equal(typewriter.getShown(), target)
  assert.equal(frames.length, 1)
})

test('fractional credit respects slow rates instead of forcing one character per frame', (t) => {
  const { clock, frames, typewriter } = typewriterFixture(t, {
    minCharsPerSecond: 5,
    maxCharsPerSecond: 5,
  })
  typewriter.setTarget('x'.repeat(100))
  for (let frame = 0; frame < 120; frame += 1) clock.tick()
  assert.equal(typewriter.getShown().length, 5)
  assert.equal(frames.length, 5)
})

test('default typewriter uses batch snap for large backlogs', (t) => {
  const { clock, frames, typewriter } = typewriterFixture(t)
  typewriter.setTarget('x'.repeat(1_000))
  clock.tick(50)
  assert.equal(frames.length, 1)
  assert.equal(frames[0].text.length, 1_000)
})

test('default snap and flush calibrate immediately', async (t) => {
  const { clock, frames, typewriter } = typewriterFixture(t)
  typewriter.setTarget('x'.repeat(1_000))
  clock.tick(50)
  assert.equal(frames.at(-1).text, 'x'.repeat(1_000))
  typewriter.setTarget('final correction', 'final')
  const drained = typewriter.drain()
  typewriter.flush()
  assert.equal(await drained, true)
  assert.equal(clock.pending, 0)
  assert.equal(frames.at(-1).text, 'final correction')
  assert.equal(frames.at(-1).activityAt, 'final')
})

test('rewrites and truncation preserve complete non-BMP characters', (t) => {
  const { clock, frames, typewriter } = typewriterFixture(t, {
    minCharsPerSecond: 30,
    maxCharsPerSecond: 30,
  })
  typewriter.setTarget('head \u{1f4a1} original')
  typewriter.flush()
  const target = 'head ' + '\u{1f4a2}'.repeat(8)
  typewriter.setTarget(target)
  for (let frame = 0; frame < 40; frame += 1) clock.tick()
  assert.equal(typewriter.getShown(), target)
  for (const frame of frames.slice(1)) {
    assert.equal(frame.text.isWellFormed(), true)
    assert.ok(target.startsWith(frame.text))
  }
  typewriter.setTarget('head ')
  clock.tick(50)
  assert.equal(typewriter.getShown(), 'head ')
  typewriter.setTarget('')
  clock.tick(50)
  assert.equal(typewriter.getShown(), '')
})

test('a split trailing surrogate waits for the next delta without spinning', (t) => {
  const { clock, frames, typewriter } = typewriterFixture(t, {
    minCharsPerSecond: 30,
    maxCharsPerSecond: 30,
  })
  typewriter.setTarget('A\ud83d')
  clock.tick(50)
  assert.equal(typewriter.getShown(), 'A')
  assert.equal(clock.pending, 0)
  typewriter.setTarget('A\u{1f4a1}B')
  clock.tick(50)
  assert.equal(typewriter.getShown(), 'A\u{1f4a1}')
  clock.tick(50)
  assert.equal(typewriter.getShown(), 'A\u{1f4a1}B')
  assert.ok(frames.every(({ text }) => text.isWellFormed()))
})

test('hidden streaming pauses and resumes without accumulating a catch-up jump', (t) => {
  const page = new EventTarget()
  page.visibilityState = 'visible'
  setGlobal(t, 'document', page)
  const { clock, frames, typewriter } = typewriterFixture(t, {
    snapRemaining: Number.POSITIVE_INFINITY,
  })
  typewriter.setTarget('x'.repeat(5_000))
  clock.tick(50)
  page.visibilityState = 'hidden'
  page.dispatchEvent(new Event('visibilitychange'))
  assert.equal(clock.pending, 0)
  typewriter.setTarget('x'.repeat(6_000))
  clock.tick(60_000)
  assert.equal(frames.length, 1)
  page.visibilityState = 'visible'
  page.dispatchEvent(new Event('visibilitychange'))
  clock.tick(50)
  assert.equal(typewriter.getShown().length, 6_000)
  assert.equal(frames.length, 2)
})

for (const hideDuringDrain of [false, true]) {
  test(`hidden completion releases drain without waiting for visibility (${hideDuringDrain})`, async (t) => {
    const page = new EventTarget()
    page.visibilityState = hideDuringDrain ? 'visible' : 'hidden'
    setGlobal(t, 'document', page)
    const { clock, frames, typewriter } = typewriterFixture(t, {
      snapRemaining: Number.POSITIVE_INFINITY,
    })
    typewriter.setTarget('x'.repeat(5_000))
    const drained = typewriter.drain()
    if (hideDuringDrain) {
      clock.tick(50)
      assert.equal(typewriter.getShown().length, 5_000)
      page.visibilityState = 'hidden'
      page.dispatchEvent(new Event('visibilitychange'))
    }
    assert.equal(await drained, true)
    assert.equal(typewriter.getShown(), 'x'.repeat(5_000))
    assert.equal(clock.pending, 0)
    const count = frames.length
    page.visibilityState = 'visible'
    page.dispatchEvent(new Event('visibilitychange'))
    clock.tick(1_000)
    assert.equal(frames.length, count)
  })
}

test('cancel releases every drain waiter and permanently prevents later writes', async (t) => {
  const { clock, frames, typewriter } = typewriterFixture(t)
  typewriter.setTarget('x'.repeat(5_000))
  const first = typewriter.drain()
  const second = typewriter.drain()
  typewriter.cancel()
  assert.equal(await first, false)
  assert.equal(await second, false)
  assert.equal(await typewriter.drain(), false)
  typewriter.flush()
  typewriter.setTarget('ignored')
  clock.tick(1_000)
  assert.deepEqual(frames, [])
  assert.equal(clock.pending, 0)
})

test('ownership invalidation cancels pending drain even before the next output frame', async (t) => {
  let current = true
  const { clock, frames, typewriter } = typewriterFixture(t, { isCurrent: () => current })
  typewriter.setTarget('x'.repeat(5_000))
  const drained = typewriter.drain()
  current = false
  clock.tick()
  assert.equal(await drained, false)
  assert.equal(clock.pending, 0)
  assert.deepEqual(frames, [])
})
