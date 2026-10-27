import assert from 'node:assert/strict'
import test from 'node:test'
import { isMobileDevice } from '../../src/lib/mobile-device.ts'
import {
  resolveMobileShellMode,
  moveMobileShellPane,
  swipeDeltaToDirection,
} from '../../src/components/layout/mobile-shell-layout.ts'

test('mobile navigation distinguishes tablets from desktop touch hardware', () => {
  for (const userAgent of ['Android', 'iPhone', 'iPad', 'iPod'])
    assert.equal(isMobileDevice({ userAgent, platform: '', maxTouchPoints: 0 }), true)
  assert.equal(
    isMobileDevice({ userAgent: 'Macintosh', platform: 'MacIntel', maxTouchPoints: 5 }),
    true,
    'iPad desktop browsing still uses the tablet shell',
  )
  for (const device of [
    { userAgent: 'Windows NT 10.0', platform: 'Win32', maxTouchPoints: 10 },
    { userAgent: 'X11; Linux x86_64', platform: 'Linux x86_64', maxTouchPoints: 10 },
    { userAgent: 'Macintosh', platform: 'MacIntel', maxTouchPoints: 0 },
  ])
    assert.equal(
      isMobileDevice(device),
      false,
      'Desktop touch support does not enable mobile views',
    )
})

test('mobile shell distinguishes compact and spacious viewports without orientation assumptions', () => {
  for (const [width, height] of [
    [390, 844],
    [844, 390],
    [959, 768],
    [1024, 639],
  ]) {
    assert.equal(resolveMobileShellMode(width, height), 'phone')
  }
  for (const [width, height] of [
    [960, 640],
    [1024, 768],
    [1366, 1024],
  ]) {
    assert.equal(resolveMobileShellMode(width, height), 'pad')
  }
})

test('horizontal navigation traverses sessions, conversation and workspace without wrapping', () => {
  assert.equal(moveMobileShellPane('chat', -1), 'sessions')
  assert.equal(moveMobileShellPane('chat', 1), 'context')
  assert.equal(moveMobileShellPane('sessions', 1), 'chat')
  assert.equal(moveMobileShellPane('context', -1), 'chat')
  assert.equal(moveMobileShellPane('sessions', -1), 'sessions')
  assert.equal(moveMobileShellPane('context', 1), 'context')
})

test('vertical scrolling, short gestures and ambiguous diagonals never switch panes', () => {
  for (const [dx, dy] of [
    [0, 100],
    [47, 0],
    [-47, 0],
    [90, 100],
    [-100, -90],
  ]) {
    assert.equal(swipeDeltaToDirection(dx, dy), 0)
  }
  assert.equal(swipeDeltaToDirection(-48, 0), 1)
  assert.equal(swipeDeltaToDirection(48, 0), -1)
  assert.equal(swipeDeltaToDirection(-120, 50), 1)
  assert.equal(swipeDeltaToDirection(120, -50), -1)
})
