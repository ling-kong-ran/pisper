import assert from 'node:assert/strict'
import test from 'node:test'
import { sessionContextDisplayWidth } from '../../src/features/chat/session-context-layout.ts'

test('narrow panes clamp the displayed width without rewriting the shared preferred width', () => {
  const preferred = 720
  assert.equal(sessionContextDisplayWidth(preferred, 800), 392)
  assert.equal(sessionContextDisplayWidth(preferred, 660), 280)
  assert.equal(sessionContextDisplayWidth(preferred, 1600), 720)
  assert.equal(sessionContextDisplayWidth(360, 1600), 360)
})
