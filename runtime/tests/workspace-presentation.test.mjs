import assert from 'node:assert/strict'
import test from 'node:test'
import { resolveWorkspacePresentation } from '../../src/features/chat/workspace-presentation.ts'

test('content presentation follows local pane width without changing task state', () => {
  for (const width of [0, 320, 390, 659]) {
    assert.equal(
      resolveWorkspacePresentation({ width, mobile: false, contentOpen: true }),
      'content',
    )
    assert.equal(
      resolveWorkspacePresentation({ width, mobile: false, contentOpen: false }),
      'conversation',
    )
  }
  assert.equal(
    resolveWorkspacePresentation({ width: 660, mobile: false, contentOpen: true }),
    'split',
  )
  assert.equal(
    resolveWorkspacePresentation({ width: 1200, mobile: true, contentOpen: true }),
    'content',
  )
})
