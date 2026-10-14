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

const startedAt = '2026-09-27T00:00:00.000Z'
const running = { sessionId: 'active', streaming: true, completed: false, runStartedAt: startedAt }
const finished = { ...running, streaming: false, completed: true }
const currentFile = {
  path: 'example.txt',
  status: 'modified',
  changedAt: '2026-09-27T00:00:01.000Z',
  added: 1,
  removed: 0,
  reverted: false,
  pending: true,
  snapshot: true,
}

test('finishing the active run reveals context only for file changes made in that run', () => {
  assert.equal(shouldRevealSessionContext(running, finished, [currentFile]), true)
  assert.equal(presentation({ preference: 'open' }), 'aside')
  assert.equal(presentation({ preference: 'open', mobileLayout: true }), 'sheet')
  assert.equal(shouldRevealSessionContext(finished, finished, [currentFile]), false)
  assert.equal(shouldRevealSessionContext(running, finished), false)
  assert.equal(shouldRevealSessionContext(running, finished, []), false)
  for (const file of [
    { ...currentFile, changedAt: '2026-09-26T23:59:59.999Z' },
    { ...currentFile, reverted: true },
    { ...currentFile, added: 0, removed: 0 },
    { ...currentFile, changedAt: 'invalid' },
  ]) {
    assert.equal(shouldRevealSessionContext(running, finished, [file]), false)
  }
  for (const file of [
    { ...currentFile, added: 0, removed: 1 },
    { ...currentFile, added: 0, status: 'created' },
    { ...currentFile, added: 0, status: 'deleted' },
    { ...currentFile, added: 0, snapshot: false },
    { ...currentFile, pending: false },
    { ...currentFile, changedAt: startedAt },
  ]) {
    assert.equal(shouldRevealSessionContext(running, finished, [file]), true)
  }
  for (const runStartedAt of [undefined, null, '', 'invalid']) {
    assert.equal(
      shouldRevealSessionContext(running, { ...finished, runStartedAt }, [currentFile]),
      false,
    )
  }
})

test('loading history, switching sessions and unsuccessful runs do not reveal context', () => {
  assert.equal(shouldRevealSessionContext(null, finished, [currentFile]), false)
  assert.equal(
    shouldRevealSessionContext({ ...running, runStartedAt: null }, finished, [currentFile]),
    false,
  )
  assert.equal(
    shouldRevealSessionContext(running, { ...finished, runStartedAt: '2026-09-27T00:01:00.000Z' }, [
      currentFile,
    ]),
    false,
  )
  assert.equal(
    shouldRevealSessionContext({ ...running, completed: true }, finished, [currentFile]),
    false,
  )
  assert.equal(
    shouldRevealSessionContext({ ...running, streaming: false }, finished, [currentFile]),
    false,
  )
  for (const current of [
    { ...finished, sessionId: 'other' },
    { ...finished, sessionId: '' },
    { ...finished, completed: false },
    { ...finished, streaming: true },
  ]) {
    assert.equal(shouldRevealSessionContext(running, current, [currentFile]), false)
  }
})
