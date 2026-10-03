import assert from 'node:assert/strict'
import test from 'node:test'
import { resolveSessionStreaming } from '../../src/features/chat/model/session-streaming-state.ts'

test('unloaded chats use catalog activity to show a recovering background run', () => {
  assert.equal(resolveSessionStreaming(undefined, { streaming: true }), true)
  assert.equal(
    resolveSessionStreaming({ loaded: false, streaming: false }, { streaming: true }),
    true,
  )
  assert.equal(resolveSessionStreaming({ loaded: false }, { streaming: false }), false)
  assert.equal(resolveSessionStreaming(undefined), false)
  assert.equal(resolveSessionStreaming(null, null), false)
})

test('loaded completion wins over an older catalog response that still reports streaming', () => {
  assert.equal(
    resolveSessionStreaming({ loaded: true, streaming: false }, { streaming: true }),
    false,
  )
  assert.equal(
    resolveSessionStreaming({ loaded: true, streaming: false }, { streaming: false }),
    false,
  )
  assert.equal(resolveSessionStreaming({ loaded: true, streaming: false }), false)
})

test('a locally started run wins immediately before either history or catalog catches up', () => {
  assert.equal(
    resolveSessionStreaming({ loaded: false, streaming: true }, { streaming: false }),
    true,
  )
  assert.equal(
    resolveSessionStreaming({ loaded: true, streaming: true }, { streaming: false }),
    true,
  )
  assert.equal(resolveSessionStreaming({ streaming: true }), true)
})
