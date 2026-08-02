import assert from 'node:assert/strict'
import test from 'node:test'

import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

function runtimeFor(session) {
  return {
    async getOrCreateSession() {
      return { session, modified: '' }
    },
    settingsManager: {
      getGlobalSettings: () => ({ defaultThinkingLevel: 'medium' }),
      setDefaultThinkingLevel() {},
    },
  }
}

test('session thinking state exposes only the active model capabilities', async () => {
  const session = {
    sessionId: 'session-thinking',
    thinkingLevel: 'off',
    model: { provider: 'relay', id: 'gpt-5.6-sol' },
    getAvailableThinkingLevels: () => ['off', 'xhigh', 'max'],
  }

  const state = await AgentRuntimeService.prototype.getSessionThinkingState.call(
    runtimeFor(session),
    session.sessionId,
  )

  assert.deepEqual(state, {
    id: 'session-thinking',
    thinkingLevel: 'off',
    availableLevels: ['off', 'xhigh', 'max'],
    status: 'supported',
    message: '',
    model: 'relay/gpt-5.6-sol',
  })
})

test('models without configurable thinking return an explicit unsupported state', async () => {
  const session = {
    sessionId: 'session-fixed-reasoning',
    thinkingLevel: 'off',
    model: { provider: 'relay', id: 'fixed-model' },
    getAvailableThinkingLevels: () => [],
  }

  const state = await AgentRuntimeService.prototype.getSessionThinkingState.call(
    runtimeFor(session),
    session.sessionId,
  )

  assert.equal(state.status, 'unsupported')
  assert.deepEqual(state.availableLevels, [])
  assert.match(state.message, /does not expose configurable thinking levels/)
})

test('thinking updates return the authoritative persisted state', async () => {
  const session = {
    sessionId: 'session-update-thinking',
    thinkingLevel: 'off',
    isStreaming: false,
    model: { provider: 'relay', id: 'gpt-5.6-sol' },
    getAvailableThinkingLevels: () => ['off', 'xhigh', 'max'],
    setThinkingLevel(level) {
      this.thinkingLevel = level
    },
  }

  const state = await AgentRuntimeService.prototype.setSessionThinkingLevel.call(
    runtimeFor(session),
    session.sessionId,
    'xhigh',
  )

  assert.equal(session.thinkingLevel, 'xhigh')
  assert.equal(state.thinkingLevel, 'xhigh')
  assert.equal(state.status, 'supported')
  assert.deepEqual(state.availableLevels, ['off', 'xhigh', 'max'])
})
