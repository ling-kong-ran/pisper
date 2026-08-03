import assert from 'node:assert/strict'
import test from 'node:test'

import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { ProviderPreferences } from '../runtime/provider-preferences.mjs'

function preferencesFor(session) {
  return {
    async getSession() {
      return { session, modified: '' }
    },
    getSettingsManager: () => ({
      getGlobalSettings: () => ({ defaultThinkingLevel: 'medium' }),
      setDefaultThinkingLevel() {},
    }),
    invalidateProjection() {},
  }
}

test('session thinking state exposes only the active model capabilities', async () => {
  const session = {
    sessionId: 'session-thinking',
    thinkingLevel: 'off',
    model: { provider: 'relay', id: 'gpt-5.6-sol' },
    getAvailableThinkingLevels: () => ['off', 'xhigh', 'max'],
  }

  const state = await ProviderPreferences.prototype.getSessionThinkingState.call(
    preferencesFor(session),
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

  const state = await ProviderPreferences.prototype.getSessionThinkingState.call(
    preferencesFor(session),
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

  const state = await ProviderPreferences.prototype.setSessionThinkingLevel.call(
    preferencesFor(session),
    session.sessionId,
    'xhigh',
  )

  assert.equal(session.thinkingLevel, 'xhigh')
  assert.equal(state.thinkingLevel, 'xhigh')
  assert.equal(state.status, 'supported')
  assert.deepEqual(state.availableLevels, ['off', 'xhigh', 'max'])
})

test('runtime facade delegates thinking state without exposing collaborator internals', async () => {
  const calls = []
  const runtime = {
    providerPreferences: {
      async getSessionThinkingState(id) {
        calls.push(['get', id])
        return { id, status: 'supported' }
      },
      async setSessionThinkingLevel(id, level) {
        calls.push(['set', id, level])
        return { id, thinkingLevel: level, status: 'supported' }
      },
    },
  }
  assert.deepEqual(
    await AgentRuntimeService.prototype.getSessionThinkingState.call(runtime, 'session-1'),
    { id: 'session-1', status: 'supported' },
  )
  assert.deepEqual(
    await AgentRuntimeService.prototype.setSessionThinkingLevel.call(runtime, 'session-1', 'max'),
    { id: 'session-1', thinkingLevel: 'max', status: 'supported' },
  )
  assert.deepEqual(calls, [
    ['get', 'session-1'],
    ['set', 'session-1', 'max'],
  ])
})
