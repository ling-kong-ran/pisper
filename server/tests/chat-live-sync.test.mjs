import assert from 'node:assert/strict'
import test from 'node:test'
import {
  hasActiveSessionAgents,
  shouldPollLiveSession,
} from '../../src/features/chat/live-session-sync.ts'

test('a locally owned SSE stream blocks live snapshot polling', () => {
  const active = {
    recovering: true,
    approvals: [{ id: 'approval-1' }],
    agents: [{ id: 'agent-1', status: 'running' }],
  }
  assert.equal(shouldPollLiveSession(active, { localStreamOwned: true }), false)
})

test('sessions without a local SSE owner poll only when live recovery data is needed', () => {
  assert.equal(shouldPollLiveSession({ recovering: true }), true)
  assert.equal(shouldPollLiveSession({ approvals: [{ id: 'approval-1' }] }), true)
  assert.equal(shouldPollLiveSession({ agents: [{ id: 'agent-1', status: 'queued' }] }), true)
  assert.equal(shouldPollLiveSession({ agents: [{ id: 'agent-1', status: 'completed' }] }), false)
  assert.equal(shouldPollLiveSession({}), false)
  assert.equal(hasActiveSessionAgents({ agents: [{ status: 'starting' }] }), true)
})
