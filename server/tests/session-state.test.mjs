import assert from 'node:assert/strict'
import test from 'node:test'
import {
  applySessionUpdate,
  DEFAULT_SESSION_STATE,
  insertInteractiveUserMessage,
  isTaskListActive,
  resolveQueuedInputs,
  resolveSessionTaskList,
  sessionStateChanged,
} from '../../src/lib/session-state.ts'

test('session state update bails out when nothing changed', () => {
  const previous = { ...DEFAULT_SESSION_STATE, streaming: true, error: '' }
  const same = applySessionUpdate(previous, { streaming: true, error: '' })
  assert.equal(same, previous)
  assert.equal(sessionStateChanged(previous, same), false)
})

test('session state update returns a new object when fields change', () => {
  const previous = { ...DEFAULT_SESSION_STATE, streaming: true }
  const next = applySessionUpdate(previous, (current) => ({ ...current, streaming: false }))
  assert.notEqual(next, previous)
  assert.equal(next.streaming, false)
})

test('interactive user messages remain visible before the active Agent bubble', () => {
  const messages = [
    { id: 'user-1', role: 'user', text: 'Start' },
    { id: 'agent-1', role: 'agent', text: 'Working', streaming: true },
  ]
  const steering = { id: 'user-2', role: 'user', text: 'Also update the tests' }
  assert.deepEqual(insertInteractiveUserMessage(messages, steering).map((message) => message.id), ['user-1', 'user-2', 'agent-1'])
  assert.deepEqual(insertInteractiveUserMessage([{ id: 'agent-done', role: 'agent', streaming: false }], steering).map((message) => message.id), ['agent-done', 'user-2'])
})

test('an explicit empty queue clears stale composer guidance', () => {
  const stale = [{ behavior: 'steer', text: 'Already processed' }]
  assert.equal(resolveQueuedInputs(stale, undefined), stale)
  assert.deepEqual(resolveQueuedInputs(stale, []), [])
  assert.deepEqual(resolveQueuedInputs(stale, null), [])
})

test('cleared task lists do not fall back to stale session list data', () => {
  const stale = { items: [{ id: 'old', title: 'Old plan', status: 'pending' }], updatedAt: '2026-01-01' }
  const cleared = resolveSessionTaskList({ loaded: true, taskList: null }, { taskList: stale })
  assert.equal(cleared, null)
  const fromSession = resolveSessionTaskList(undefined, { taskList: stale })
  assert.equal(fromSession, stale)
})

test('fully completed task lists hide when idle but stay visible while streaming', () => {
  const completed = { items: [{ id: 'a', title: 'Done', status: 'completed' }] }
  assert.equal(isTaskListActive(completed, { streaming: false }), false)
  assert.equal(isTaskListActive(completed, { streaming: true }), true)
  assert.equal(isTaskListActive({ items: [{ id: 'a', title: 'Todo', status: 'pending' }] }, { streaming: false }), true)
})
