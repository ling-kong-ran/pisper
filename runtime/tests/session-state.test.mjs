import assert from 'node:assert/strict'
import test from 'node:test'
import {
  applySessionUpdate,
  DEFAULT_SESSION_STATE,
  insertInteractiveUserMessage,
  isPlanActive,
  resolveMessageRunActivity,
  resolveQueuedInputs,
  resolveSessionPlan,
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

test('interactive user messages keep the current Agent bubble last between queued turns', () => {
  const messages = [
    { id: 'user-1', role: 'user', text: 'Start' },
    { id: 'agent-1', role: 'agent', text: 'Working', streaming: true },
  ]
  const steering = { id: 'user-2', role: 'user', text: 'Also update the tests' }
  assert.deepEqual(
    insertInteractiveUserMessage(messages, steering).map((message) => message.id),
    ['user-1', 'user-2', 'agent-1'],
  )
  assert.deepEqual(
    insertInteractiveUserMessage(
      [{ id: 'agent-between-turns', role: 'agent', streaming: false }],
      steering,
    ).map((message) => message.id),
    ['user-2', 'agent-between-turns'],
  )
  assert.deepEqual(insertInteractiveUserMessage([], steering), [steering])
})

test('run activity stays bound to its Agent message across appended turns and refreshes', () => {
  const firstRun = {
    thinkingText: 'Inspect the implementation.',
    tools: [{ id: 'tool-1', name: 'read', status: 'done' }],
  }
  const liveRun = {
    streaming: true,
    thinkingText: 'Apply the follow-up.',
    tools: [],
  }
  const firstAgent = {
    id: 'agent-1',
    role: 'agent',
    text: 'First answer',
    runActivity: firstRun,
  }
  const currentAgent = { id: 'agent-2', role: 'agent', text: '', streaming: true }

  assert.equal(resolveMessageRunActivity(firstAgent, false, liveRun), firstRun)
  assert.equal(resolveMessageRunActivity(currentAgent, true, liveRun), liveRun)
  assert.equal(resolveMessageRunActivity(firstAgent, true, {}), firstRun)
})

test('an explicit empty queue clears stale composer guidance', () => {
  const stale = [{ behavior: 'steer', text: 'Already processed' }]
  assert.equal(resolveQueuedInputs(stale, undefined), stale)
  assert.deepEqual(resolveQueuedInputs(stale, []), [])
  assert.deepEqual(resolveQueuedInputs(stale, null), [])
})

test('cleared plans do not fall back to stale session list data', () => {
  const stale = {
    items: [{ id: 'old', title: 'Old plan', status: 'pending' }],
    updatedAt: '2026-01-01',
  }
  const cleared = resolveSessionPlan({ loaded: true, plan: null }, { plan: stale })
  assert.equal(cleared, null)
  const fromSession = resolveSessionPlan(undefined, { plan: stale })
  assert.equal(fromSession, stale)
  assert.equal(resolveSessionPlan(undefined, { taskList: stale }), stale)
  assert.equal(resolveSessionPlan({ loaded: true, taskList: null }, { plan: stale }), null)
})

test('persisted plans stay visible after an idle runtime restart', () => {
  const completed = { items: [{ id: 'a', title: 'Done', status: 'completed' }] }
  assert.equal(isPlanActive(completed, { streaming: false }), true)
  assert.equal(isPlanActive(completed, { streaming: true }), true)
  assert.equal(isPlanActive({ items: [] }, { streaming: false }), false)
  assert.equal(isPlanActive(null, { streaming: false }), false)
})
