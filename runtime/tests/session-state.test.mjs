import assert from 'node:assert/strict'
import test from 'node:test'
import {
  applySessionUpdate,
  DEFAULT_SESSION_STATE,
  deliverQueuedInputs,
  insertInteractiveUserMessage,
  lastSettledMessageIndex,
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

test('pending queued messages sit after the live reply and delivery freezes what was written (Claude-style)', () => {
  const live = { id: 'agent-1', role: 'agent', text: '第一段。第二段。', streaming: true }
  let messages = [{ id: 'user-1', role: 'user', text: '写一段很长的说明' }, live]
  const pending = (id, text) => ({
    id: `q-${id}`,
    role: 'user',
    text,
    queuedInputId: id,
    queuedPending: true,
  })
  // 还没送达：排在正在写的回复后面，正在写的回复仍是“最新一条”。
  messages = insertInteractiveUserMessage(messages, pending('a', '补充：列出风险'))
  messages = insertInteractiveUserMessage(messages, pending('b', '立即发送这条'))
  assert.deepEqual(
    messages.map((m) => m.id),
    ['user-1', 'agent-1', 'q-a', 'q-b'],
  )
  assert.equal(lastSettledMessageIndex(messages), 1)
  // 送达（立即发送 b）：已写出的回复定格成一段并标“已打断”，b 接在后面，正在写的回复从空白接着写；a 仍在末尾排队。
  const state = deliverQueuedInputs(
    { ...DEFAULT_SESSION_STATE, messages },
    { agentId: 'agent-1', deliveredIds: ['b'], frozenText: '第一段。第二段。', interrupted: true },
  )
  assert.deepEqual(
    state.messages.map((m) => [
      m.id,
      m.role,
      m.text,
      Boolean(m.queuedPending),
      Boolean(m.interrupted),
    ]),
    [
      ['user-1', 'user', '写一段很长的说明', false, false],
      ['agent-1-part-1', 'agent', '第一段。第二段。', false, true],
      ['q-b', 'user', '立即发送这条', false, false],
      ['agent-1', 'agent', '', false, false],
      ['q-a', 'user', '补充：列出风险', true, false],
    ],
  )
  assert.deepEqual(state.deliveredInputIds, ['b'])
  assert.equal(lastSettledMessageIndex(state.messages), 3)
  // 再送达 a（排队，不是立即发送）：第二段回复定格，不标“已打断”。
  const next = deliverQueuedInputs(
    {
      ...state,
      messages: state.messages.map((m) =>
        m.id === 'agent-1' ? { ...m, text: '重写后的回复。' } : m,
      ),
    },
    { agentId: 'agent-1', deliveredIds: ['a'], frozenText: '重写后的回复。', interrupted: false },
  )
  assert.deepEqual(
    next.messages.map((m) => m.id),
    ['user-1', 'agent-1-part-1', 'q-b', 'agent-1-part-2', 'q-a', 'agent-1'],
  )
  assert.equal(next.messages[3].interrupted, undefined)
  // HTTP 响应晚于送达：气泡直接按已送达插到正在写的回复之前。
  const late = insertInteractiveUserMessage(next.messages, {
    id: 'q-c',
    role: 'user',
    text: '晚到的',
    queuedInputId: 'c',
    queuedPending: false,
  })
  assert.deepEqual(
    late.slice(-2).map((m) => m.id),
    ['q-c', 'agent-1'],
  )
  // 没写出任何字就送达：不留空白的定格段。
  const empty = deliverQueuedInputs(
    { ...DEFAULT_SESSION_STATE, messages: [live, pending('d', 'x')] },
    { agentId: 'agent-1', deliveredIds: ['d'], frozenText: '', interrupted: true },
  )
  assert.deepEqual(
    empty.messages.map((m) => m.id),
    ['q-d', 'agent-1'],
  )
})
