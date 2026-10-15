import assert from 'node:assert/strict'
import test from 'node:test'
import {
  resolveMobileShellMode,
  moveMobileShellPane,
  swipeDeltaToDirection,
} from '../../src/components/layout/mobile-shell-layout.ts'

test('mobile shell keeps compact and short landscape viewports on one pane', () => {
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
