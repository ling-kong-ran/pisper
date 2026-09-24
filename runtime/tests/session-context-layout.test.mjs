import assert from 'node:assert/strict'
import test from 'node:test'
import {
  normalizeSessionContextWidth,
  resolveSessionContextPresentation,
  shouldRevealSessionContext,
} from '../../src/features/chat/session-context-layout.ts'

function presentation(overrides = {}) {
  return resolveSessionContextPresentation({
    availableWidth: 1200,
    mobileLayout: false,
    hasSession: true,
    preference: 'auto',
    ...overrides,
  })
}

test('wide chat workspaces show one contextual rail until the user closes it', () => {
  assert.equal(presentation(), 'aside')
  assert.equal(presentation({ availableWidth: 800 }), 'aside')
  assert.equal(presentation({ preference: 'closed' }), 'closed')
  assert.equal(presentation({ hasSession: false }), 'closed')
})

test('narrow and mobile workspaces keep the conversation visible until context is requested', () => {
  assert.equal(presentation({ availableWidth: 799 }), 'closed')
  assert.equal(presentation({ availableWidth: 799, preference: 'open' }), 'sheet')
  assert.equal(presentation({ mobileLayout: true }), 'closed')
  assert.equal(presentation({ mobileLayout: true, preference: 'open' }), 'sheet')
  assert.equal(presentation({ mobileLayout: true, preference: 'closed' }), 'closed')
})

test('explicit context preference survives a width change', () => {
  assert.equal(presentation({ availableWidth: 799, preference: 'open' }), 'sheet')
  assert.equal(presentation({ availableWidth: 1200, preference: 'open' }), 'aside')
  assert.equal(presentation({ availableWidth: 799, preference: 'closed' }), 'closed')
  assert.equal(presentation({ availableWidth: 1200, preference: 'closed' }), 'closed')
})

test('context width restores valid pixels and bounds oversized or narrow preferences', () => {
  assert.equal(normalizeSessionContextWidth(540), 540)
  assert.equal(normalizeSessionContextWidth(479.6), 480)
  assert.equal(normalizeSessionContextWidth(479.4), 479)
  assert.equal(normalizeSessionContextWidth(0), 280)
  assert.equal(normalizeSessionContextWidth(-10), 280)
  assert.equal(normalizeSessionContextWidth(10000), 720)
})

test('invalid context width preferences fall back to the initial width', () => {
  for (const value of [undefined, null, '480', '', true, {}, [], NaN, Infinity, -Infinity]) {
    assert.equal(normalizeSessionContextWidth(value), 360)
  }
})

test('finishing the active run reveals file context even after the panel was closed', () => {
  const running = { sessionId: 'active', streaming: true, completed: false }
  const finished = { sessionId: 'active', streaming: false, completed: true }
  assert.equal(shouldRevealSessionContext(running, finished), true)
  assert.equal(presentation({ preference: 'open' }), 'aside')
  assert.equal(presentation({ preference: 'open', mobileLayout: true }), 'sheet')
  assert.equal(shouldRevealSessionContext(finished, finished), false)
})

test('loading history, switching sessions and unsuccessful runs do not reveal context', () => {
  const running = { sessionId: 'active', streaming: true, completed: false }
  const finished = { sessionId: 'active', streaming: false, completed: true }
  assert.equal(shouldRevealSessionContext(null, finished), false)
  assert.equal(shouldRevealSessionContext({ ...running, streaming: false }, finished), false)
  assert.equal(shouldRevealSessionContext(running, { ...finished, sessionId: 'other' }), false)
  assert.equal(shouldRevealSessionContext(running, { ...finished, sessionId: '' }), false)
  assert.equal(shouldRevealSessionContext(running, { ...finished, completed: false }), false)
  assert.equal(shouldRevealSessionContext(running, { ...finished, streaming: true }), false)
})
