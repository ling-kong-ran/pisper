import assert from 'node:assert/strict'
import test from 'node:test'
import { createPrimaryActionRegistry } from '../../src/app/primary-action.ts'
import { mergeSessionLists, removeTiledSession, toggleTiledSession } from '../../src/features/chat/session-list.ts'

test('primary action remains callable until its page registration is disposed', () => {
  const registry = createPrimaryActionRegistry()
  let calls = 0
  const dispose = registry.register(() => { calls += 1 })

  registry.invoke()
  registry.invoke()
  assert.equal(calls, 2)

  dispose()
  registry.invoke()
  assert.equal(calls, 2)
})

test('a queued primary action runs once when a lazy page registers', () => {
  const registry = createPrimaryActionRegistry()
  let calls = 0

  registry.invoke()
  registry.register(() => { calls += 1 })
  assert.equal(calls, 1)
})

test('disposing an old page action does not clear the newly registered action', () => {
  const registry = createPrimaryActionRegistry()
  let calls = 0
  const disposeOld = registry.register(() => {})
  registry.register(() => { calls += 1 })

  disposeOld()
  registry.invoke()
  assert.equal(calls, 1)
})

test('stale initial session lists preserve an optimistically created session', () => {
  const optimistic = { id: 'new-session', name: '新会话' }
  const stale = [{ id: 'existing-session', name: '旧会话' }]

  assert.deepEqual(mergeSessionLists([optimistic], stale), [stale[0], optimistic])
})

test('removing a tiled session keeps the session itself available elsewhere', () => {
  assert.deepEqual(removeTiledSession(['first', 'second', 'third'], 'second'), ['first', 'third'])
})

test('a session can be added to and removed from the tiled set', () => {
  assert.deepEqual(toggleTiledSession(['first'], 'second'), ['first', 'second'])
  assert.deepEqual(toggleTiledSession(['first', 'second'], 'second'), ['first'])
})
