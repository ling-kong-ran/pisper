import assert from 'node:assert/strict'
import test from 'node:test'
import {
  AGENT_SESSION_EVENT_CHANNELS,
  bridgeAgentSessionEvent,
  finishAgentLifecycle,
  initialAgentLifecycle,
} from '../runtime/agent-event-bridge.mjs'

const EXPECTED_AGENT_SESSION_EVENTS = [
  'agent_start',
  'agent_end',
  'agent_settled',
  'turn_start',
  'turn_end',
  'message_start',
  'message_update',
  'message_end',
  'tool_execution_start',
  'tool_execution_update',
  'tool_execution_end',
  'queue_update',
  'compaction_start',
  'compaction_end',
  'auto_retry_start',
  'auto_retry_end',
  'entry_appended',
  'session_info_changed',
  'thinking_level_changed',
]

test('AgentSession event matrix assigns every Pi event to a safe product channel', () => {
  assert.deepEqual(
    Object.keys(AGENT_SESSION_EVENT_CHANNELS).sort(),
    EXPECTED_AGENT_SESSION_EVENTS.sort(),
  )
  assert.equal(AGENT_SESSION_EVENT_CHANNELS.message_update, 'text_or_thinking_patch')
  assert.equal(AGENT_SESSION_EVENT_CHANNELS.entry_appended, 'session_tree_changed')
  assert.equal(AGENT_SESSION_EVENT_CHANNELS.agent_settled, 'agent_lifecycle')
})

test('AgentSession lifecycle bridge projects boundaries, retry completion, and settlement', () => {
  const events = []
  const live = {
    lastActivityAt: '2026-08-03T00:00:00.000Z',
    lifecycle: initialAgentLifecycle('2026-08-03T00:00:00.000Z'),
    currentActivity: null,
  }
  const emit = (event, data) => events.push({ event, data })
  const bridge = (event, second) => {
    live.lastActivityAt = second || live.lastActivityAt
    return bridgeAgentSessionEvent(event, live, emit)
  }

  bridge({ type: 'agent_start' })
  bridge({ type: 'turn_start' }, '2026-08-03T00:00:01.000Z')
  bridge({ type: 'message_start', message: { role: 'assistant' } })
  bridge({
    type: 'message_update',
    message: { role: 'assistant' },
    assistantMessageEvent: { type: 'text_start' },
  })
  const beforeDelta = events.length
  bridge({
    type: 'message_update',
    message: { role: 'assistant' },
    assistantMessageEvent: { type: 'text_delta', delta: 'private streamed content' },
  })
  assert.equal(events.length, beforeDelta)
  assert.equal(live.lifecycle.phase, 'responding')

  bridge({
    type: 'message_end',
    message: { role: 'assistant', stopReason: 'toolUse' },
  })
  const beforeTool = events.length
  bridge({
    type: 'tool_execution_start',
    toolCallId: 'call-1',
    toolName: 'read',
    args: { secret: true },
  })
  bridge({
    type: 'tool_execution_update',
    toolCallId: 'call-1',
    toolName: 'read',
    args: { secret: true },
    partialResult: { content: 'private tool output' },
  })
  bridge({
    type: 'tool_execution_end',
    toolCallId: 'call-1',
    toolName: 'read',
    result: { private: true },
    isError: false,
  })
  assert.equal(events.length, beforeTool)
  assert.deepEqual(live.lifecycle.tool, { id: 'call-1', name: 'read', error: false })

  bridge({ type: 'turn_end', message: { role: 'assistant' }, toolResults: [] })
  bridge({
    type: 'auto_retry_start',
    attempt: 1,
    maxAttempts: 3,
    delayMs: 1000,
    errorMessage: 'temporary failure',
  })
  assert.equal(live.lifecycle.phase, 'retrying')
  bridge({ type: 'agent_end', messages: [], willRetry: true })
  bridge({ type: 'auto_retry_end', success: true, attempt: 1 })
  assert.equal(live.lifecycle.phase, 'processing_result')
  assert.deepEqual(live.lifecycle.retry, { attempt: 1, success: true })
  bridge({ type: 'agent_end', messages: [], willRetry: false })
  assert.equal(live.lifecycle.phase, 'settling')
  bridge({ type: 'agent_settled' })
  assert.equal(live.lifecycle.phase, 'settled')
  assert.equal(live.lifecycle.turn, 1)
  assert.equal(live.currentActivity.stage, 'finalizing')

  const lifecycleEvents = events.filter((item) => item.event === 'agent_lifecycle')
  assert.ok(lifecycleEvents.some((item) => item.data.lifecycle.event === 'message_start'))
  assert.ok(lifecycleEvents.some((item) => item.data.lifecycle.event === 'auto_retry_end'))
  assert.equal(lifecycleEvents.at(-1).data.lifecycle.event, 'agent_settled')
  assert.doesNotMatch(
    JSON.stringify(lifecycleEvents),
    /private streamed content|private tool output|secret/,
  )
})

test('AgentSession metadata bridge hides custom entry data and normalizes public fields', () => {
  const events = []
  const live = { lastActivityAt: '2026-08-03T00:00:00.000Z' }
  const emit = (event, data) => events.push({ event, data })

  bridgeAgentSessionEvent(
    {
      type: 'entry_appended',
      entry: {
        type: 'custom',
        id: 'entry-1',
        parentId: 'entry-0',
        customType: 'private.extension.record',
        data: { apiKey: 'must-not-leak' },
      },
    },
    live,
    emit,
  )
  bridgeAgentSessionEvent({ type: 'session_info_changed', name: '  Session\nname  ' }, live, emit)
  bridgeAgentSessionEvent({ type: 'thinking_level_changed', level: 'high' }, live, emit)

  assert.deepEqual(events[0], {
    event: 'session_tree_changed',
    data: {
      revision: 1,
      entry: { id: 'entry-1', parentId: 'entry-0', kind: 'runtime' },
    },
  })
  assert.deepEqual(events[1], {
    event: 'session_title',
    data: { name: 'Session name', source: 'pi_session' },
  })
  assert.deepEqual(events[2], {
    event: 'thinking_level_changed',
    data: { level: 'high' },
  })
  assert.doesNotMatch(JSON.stringify(events), /must-not-leak|private\.extension/)
})

test('runtime terminal lifecycle keeps a bounded public error', () => {
  const completed = finishAgentLifecycle(
    { phase: 'settled', event: 'agent_settled', turn: 2 },
    '',
    '2026-08-03T00:00:02.000Z',
  )
  assert.deepEqual(completed, {
    phase: 'completed',
    event: 'runtime_done',
    turn: 2,
    updatedAt: '2026-08-03T00:00:02.000Z',
  })

  const failed = finishAgentLifecycle(
    null,
    `failure\n${'x'.repeat(400)}`,
    '2026-08-03T00:00:03.000Z',
  )
  assert.equal(failed.phase, 'failed')
  assert.equal(failed.event, 'runtime_error')
  assert.ok(failed.message.length <= 240)
  assert.doesNotMatch(failed.message, /\n/)
})
