import assert from 'node:assert/strict'
import test from 'node:test'
import {
  chatLayoutAppearanceStyle,
  chatLayoutMeasurementKey,
  orderChatLayoutSlots,
} from '../../src/features/chat/layout/chat-layout-appearance.ts'
import { DEFAULT_CHAT_LAYOUT } from '../../src/features/chat/layout/chat-layout.ts'
import { resolveAnchoredPopupLayout } from '../../src/features/chat/anchored-popup-layout.ts'

test('default chat layout inherits application typography and theme tokens', () => {
  const style = chatLayoutAppearanceStyle(DEFAULT_CHAT_LAYOUT.desktop, 'inherit')
  assert.equal(style.fontFamily, undefined)
  assert.equal(style['--app-message-font-size'], undefined)
  assert.equal(style['--brand-blue'], undefined)
  assert.equal(style['--user-bubble-text'], undefined)
  assert.equal(style['--chat-content-width'], '768px')
})

test('chat typography changes stay scoped and preserve semantic colors', () => {
  const style = chatLayoutAppearanceStyle(
    {
      ...DEFAULT_CHAT_LAYOUT.mobile,
      fontFamily: 'serif',
      fontSize: 20,
      contentWidth: 600,
      density: 'compact',
      messageStyle: 'plain',
    },
    'teal',
  )
  assert.match(style.fontFamily, /serif/)
  assert.equal(style['--app-message-font-size'], '20px')
  assert.equal(style['--chat-content-width'], '600px')
  assert.equal(style['--user-bubble-text'], 'var(--text)')
  assert.equal(style['--text'], undefined)
  assert.equal(style['--danger'], undefined)
  assert.ok(style['--chat-layout-accent-light'])
  assert.ok(style['--chat-layout-accent-dark'])
  assert.equal(DEFAULT_CHAT_LAYOUT.mobile.fontSize, null)
})

test('layout reordering preserves both slot identities and follows the visual reading order', () => {
  const transcript = { key: 'transcript', scrollTop: 900 }
  const composer = { key: 'composer', draft: 'unsent draft', attachments: ['image.png'] }
  const slots = [transcript, composer]
  const top = orderChatLayoutSlots('top', slots)
  assert.equal(top[0], composer)
  assert.equal(top[1], transcript)
  const bottom = orderChatLayoutSlots('bottom', slots)
  assert.equal(bottom[0], transcript)
  assert.equal(bottom[1], composer)
  assert.deepEqual(slots, [transcript, composer])
})

test('only changes affecting row measurements invalidate transcript layout caches', () => {
  const appearance = DEFAULT_CHAT_LAYOUT.desktop
  const original = chatLayoutMeasurementKey(appearance)
  for (const changes of [
    { contentWidth: 600 },
    { fontSize: 20 },
    { fontFamily: 'serif' },
    { density: 'compact' },
    { messageStyle: 'plain' },
  ]) {
    assert.notEqual(chatLayoutMeasurementKey({ ...appearance, ...changes }), original)
  }
  assert.equal(
    chatLayoutMeasurementKey({ ...appearance, composerPosition: 'top', showUsage: false }),
    original,
  )
})

test('top composer menus flip down and bottom composer menus retain their upward placement', () => {
  const viewport = { top: 0, left: 0, width: 900, height: 700 }
  const menu = { width: 270, height: 340 }
  const top = resolveAnchoredPopupLayout({
    anchor: { top: 110, bottom: 150, left: 130, right: 174 },
    menu,
    viewport,
  })
  assert.equal(top.side, 'bottom')
  assert.equal(top.top, 158)
  const bottom = resolveAnchoredPopupLayout({
    anchor: { top: 580, bottom: 624, left: 130, right: 174 },
    menu,
    viewport,
  })
  assert.equal(bottom.side, 'top')
  assert.equal(bottom.top, 232)
})

test('menus fit the visible keyboard viewport and never force a minimum height outside it', () => {
  const viewport = { top: 200, left: 20, width: 320, height: 240 }
  const layout = resolveAnchoredPopupLayout({
    anchor: { top: 290, bottom: 334, left: 300, right: 344 },
    menu: { width: 600, height: 900 },
    viewport,
    maxHeight: 330,
    align: 'end',
  })
  assert.equal(layout.side, 'bottom')
  assert.equal(layout.maxHeight, 90)
  assert.equal(layout.width, 304)
  assert.ok(layout.left >= viewport.left + 8)
  assert.ok(layout.left + layout.width <= viewport.left + viewport.width - 8)
  assert.ok(layout.top >= viewport.top + 8)
  assert.ok(layout.top + layout.maxHeight <= viewport.top + viewport.height - 8)
})

test('preferred placement stays stable when enough room exists for a capped menu', () => {
  const layout = resolveAnchoredPopupLayout({
    anchor: { top: 400, bottom: 444, left: 30, right: 74 },
    menu: { width: 300, height: 1000 },
    viewport: { top: 0, left: 0, width: 900, height: 900 },
    placement: 'top',
    maxHeight: 330,
  })
  assert.equal(layout.side, 'top')
  assert.equal(layout.maxHeight, 330)
  assert.equal(layout.top, 62)
})
