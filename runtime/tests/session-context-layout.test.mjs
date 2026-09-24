import assert from 'node:assert/strict'
import test from 'node:test'
import { resolveSessionContextPresentation } from '../../src/features/chat/session-context-layout.ts'

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
