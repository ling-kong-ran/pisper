import assert from 'node:assert/strict'
import test from 'node:test'
import { createScrollFollowController } from '../../src/lib/scroll-follow.ts'

function fixture(t, { reducedMotion, threshold, rounded = false } = {}) {
  let time = 0
  let nextId = 0
  const frames = new Map()
  const writes = []
  const unread = []
  const document = new EventTarget()
  const media = { matches: false }
  document.documentElement = { dataset: {} }
  document.defaultView = { matchMedia: () => media }
  const node = new EventTarget()
  Object.assign(node, {
    ownerDocument: document,
    scrollTop: 700,
    scrollHeight: 1000,
    clientHeight: 300,
    scrollTo({ top, behavior }) {
      assert.equal(behavior, 'instant')
      this.scrollTop = Math.max(
        0,
        Math.min(this.scrollHeight - this.clientHeight, rounded ? Math.round(top) : top),
      )
      writes.push(this.scrollTop)
      this.dispatchEvent(new Event('scroll'))
    },
  })
  const controller = createScrollFollowController(node, {
    reducedMotion,
    threshold,
    onUnreadChange: (value) => unread.push(value),
    requestFrame(callback) {
      const id = ++nextId
      frames.set(id, callback)
      return id
    },
    cancelFrame(id) {
      frames.delete(id)
    },
  })
  t.after(() => controller.dispose())
  const event = (type, props = {}, target = node) => {
    target.dispatchEvent(Object.assign(new Event(type), props))
  }
  const tick = (elapsed = 1000 / 60) => {
    time += elapsed
    const pending = [...frames.values()]
    frames.clear()
    pending.forEach((callback) => callback(time))
  }
  const settle = () => {
    let count = 0
    while (frames.size && count++ < 300) tick()
    assert.equal(frames.size, 0, 'animation must converge and release its final frames')
  }
  const scroll = (top) => {
    node.scrollTop = top
    event('scroll')
  }
  return { controller, node, document, media, event, tick, settle, scroll, frames, writes, unread }
}

test('content bursts and virtual measurements coalesce into one continuous follow animation', (t) => {
  const f = fixture(t)
  f.node.scrollHeight += 300
  for (let i = 0; i < 30; i += 1) {
    f.controller.contentChanged()
    f.controller.maintainBottom()
  }
  assert.equal(f.frames.size, 1)
  assert.deepEqual(f.writes, [])
  f.tick()
  assert.ok(f.node.scrollTop > 700 && f.node.scrollTop < 1000)
  const firstTop = f.node.scrollTop
  f.node.scrollHeight += 250
  f.controller.contentChanged()
  assert.equal(f.frames.size, 1)
  f.tick()
  assert.ok(f.node.scrollTop > firstTop && f.node.scrollTop < 1250)
  f.settle()
  assert.equal(f.node.scrollTop, 1250)
  assert.ok(f.writes.every((top, index) => index === 0 || top >= f.writes[index - 1]))
  assert.deepEqual(f.unread, [])
})

test('follow speed is time-corrected across 60Hz and 120Hz displays', (t) => {
  const sixty = fixture(t)
  const fast = fixture(t)
  for (const f of [sixty, fast]) {
    f.node.scrollHeight += 2000
    f.controller.contentChanged()
    f.tick(0)
  }
  for (let i = 0; i < 10; i += 1) sixty.tick(1000 / 60)
  for (let i = 0; i < 20; i += 1) fast.tick(1000 / 120)
  assert.ok(Math.abs(sixty.node.scrollTop - fast.node.scrollTop) < 0.001)
})

test('small upward wheel intent cancels queued frames even inside the bottom threshold', (t) => {
  const f = fixture(t)
  f.node.scrollHeight += 20
  f.controller.maintainBottom()
  f.event('wheel', { deltaX: 0, deltaY: -8, ctrlKey: false })
  assert.equal(f.frames.size, 0)
  f.scroll(692)
  f.controller.contentChanged()
  f.controller.maintainBottom()
  f.settle()
  assert.equal(f.node.scrollTop, 692)
  assert.deepEqual(f.unread, [true])
  f.scroll(720)
  f.settle()
  assert.deepEqual(f.unread, [true, false])
  f.node.scrollHeight += 100
  f.controller.contentChanged()
  f.settle()
  assert.equal(f.node.scrollTop, 820)
})

test('upward intent cancels an active animation and its delayed programmatic scroll events', (t) => {
  const f = fixture(t)
  f.node.scrollHeight += 500
  f.controller.contentChanged()
  f.tick()
  f.event('wheel', { deltaX: 0, deltaY: -10, ctrlKey: false })
  const top = f.node.scrollTop
  f.event('scroll')
  f.controller.contentChanged()
  f.settle()
  assert.equal(f.node.scrollTop, top)
  assert.deepEqual(f.unread, [true])
})

test('return-to-bottom animates to the latest growing target and clears unread', (t) => {
  const f = fixture(t)
  f.controller.pauseFollowing()
  f.scroll(100)
  f.controller.contentChanged()
  f.controller.scrollToBottom('smooth')
  assert.equal(f.node.scrollTop, 100)
  f.tick()
  assert.ok(f.node.scrollTop > 100 && f.node.scrollTop < 700)
  f.node.scrollHeight += 200
  f.settle()
  assert.equal(f.node.scrollTop, 900)
  assert.deepEqual(f.unread, [true, false])
})

test('touching without scrolling only suspends animation, while dragging upward detaches it', (t) => {
  const f = fixture(t)
  f.node.scrollHeight += 40
  f.controller.contentChanged()
  f.event('touchstart', { touches: [{ clientX: 50, clientY: 100 }] })
  f.tick()
  assert.equal(f.node.scrollTop, 700)
  f.event('touchend')
  f.settle()
  assert.equal(f.node.scrollTop, 740)
  f.event('touchstart', { touches: [{ clientX: 50, clientY: 100 }] })
  f.event('touchmove', { touches: [{ clientX: 50, clientY: 108 }] })
  f.scroll(732)
  f.event('touchend')
  f.controller.contentChanged()
  f.settle()
  assert.equal(f.node.scrollTop, 732)
  assert.deepEqual(f.unread, [true])
})

test('scrollbar dragging and keyboard up detach follow, but ordinary clicks do not', (t) => {
  const f = fixture(t)
  f.node.scrollHeight += 100
  f.controller.contentChanged()
  f.event('pointerdown', { pointerType: 'mouse', button: 0 })
  f.tick()
  assert.equal(f.node.scrollTop, 700)
  f.event('pointerup', {}, f.document)
  f.settle()
  assert.equal(f.node.scrollTop, 800)
  f.event('pointerdown', { pointerType: 'mouse', button: 0 })
  f.scroll(760)
  f.event('pointerup', {}, f.document)
  f.controller.contentChanged()
  f.settle()
  assert.equal(f.node.scrollTop, 760)
  f.controller.scrollToBottom()
  f.event('keydown', { key: 'ArrowUp' })
  f.controller.contentChanged()
  f.settle()
  assert.equal(f.frames.size, 0)
  assert.deepEqual(f.unread, [true, false, true])
})

test('horizontal wheel gestures and control-wheel zoom do not disable following', (t) => {
  const f = fixture(t)
  f.node.scrollHeight += 100
  f.controller.contentChanged()
  f.event('wheel', { deltaX: 30, deltaY: -4, ctrlKey: false })
  f.event('wheel', { deltaX: 0, deltaY: -30, ctrlKey: true })
  f.settle()
  assert.equal(f.node.scrollTop, 800)
  assert.deepEqual(f.unread, [])
})

test('explicit initial positioning is immediate and respects real scrollable height', (t) => {
  const f = fixture(t)
  f.scroll(0)
  f.controller.scrollToBottom()
  assert.equal(f.node.scrollTop, 700)
  f.settle()
  f.node.scrollHeight = 200
  f.controller.maintainBottom()
  f.settle()
  assert.equal(f.node.scrollTop, 0)
})

test('reduced motion follows app overrides and can change during an animation', (t) => {
  const f = fixture(t)
  f.media.matches = true
  f.node.scrollHeight += 200
  f.controller.contentChanged()
  f.tick()
  assert.equal(f.node.scrollTop, 900)
  f.document.documentElement.dataset.motion = 'full'
  f.node.scrollHeight += 200
  f.controller.contentChanged()
  f.tick()
  assert.ok(f.node.scrollTop > 900 && f.node.scrollTop < 1100)
  f.document.documentElement.dataset.motion = 'reduced'
  f.tick()
  assert.equal(f.node.scrollTop, 1100)
  f.settle()
})

test('a late layout change after arrival rechecks the target before releasing follow', (t) => {
  const f = fixture(t, { reducedMotion: () => true })
  f.node.scrollHeight += 100
  f.controller.contentChanged()
  f.tick()
  assert.equal(f.node.scrollTop, 800)
  f.node.scrollHeight += 30
  f.settle()
  assert.equal(f.node.scrollTop, 830)
})

test('rounded WebView scroll offsets converge and do not leave a perpetual frame loop', (t) => {
  const f = fixture(t, { rounded: true })
  f.node.scrollHeight += 17
  f.controller.contentChanged()
  f.settle()
  assert.equal(f.node.scrollTop, 717)
})

test('history anchoring and target navigation can pause follow before a programmatic jump', (t) => {
  const f = fixture(t)
  f.node.scrollHeight += 300
  f.controller.contentChanged()
  f.controller.pauseFollowing()
  f.scroll(200)
  f.node.scrollHeight += 500
  f.scroll(700)
  f.controller.maintainBottom()
  f.settle()
  assert.equal(f.node.scrollTop, 700)
})

test('dispose releases animation frames and input listeners without late writes', (t) => {
  const f = fixture(t)
  f.node.scrollHeight += 300
  f.controller.contentChanged()
  f.tick()
  const top = f.node.scrollTop
  f.controller.dispose()
  assert.equal(f.frames.size, 0)
  f.event('wheel', { deltaX: 0, deltaY: 100, ctrlKey: false })
  f.event('pointerup', {}, f.document)
  f.controller.contentChanged()
  f.controller.maintainBottom()
  f.controller.scrollToBottom()
  f.settle()
  assert.equal(f.node.scrollTop, top)
  assert.deepEqual(f.unread, [])
})
