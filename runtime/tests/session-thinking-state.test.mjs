import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { sessionRuntimeRoutes } from '../http/routes/sessions-runtime.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  ProviderPreferences,
  clampThinkingLevelToAvailable,
  reconcileSessionThinkingLevel,
} from '../runtime/provider-preferences.mjs'

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

async function emptyConfigurationFixture(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-empty-thinking-'))
  const runtimes = new Set()
  const settings = Object.freeze({ defaultThinkingLevel: 'medium' })
  const create = () => {
    const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
    runtime.settingsManager = { getGlobalSettings: () => settings }
    runtime.createSessionRuntime = async () => assert.fail('read must not materialize a session')
    runtime.providerPreferences.resolveSessionModel = async () =>
      assert.fail('empty model selection must not resolve a model')
    runtimes.add(runtime)
    return runtime
  }
  const close = async (runtime) => {
    await runtime.dispose()
    runtimes.delete(runtime)
  }
  t.after(async () => {
    for (const runtime of runtimes) await close(runtime)
    await rm(directory, { recursive: true, force: true })
  })
  return { directory, settings, create, close }
}

test('thinking reads for an unconfigured first-run session stay empty before and after restart', async (t) => {
  const fixture = await emptyConfigurationFixture(t)
  const runtime = fixture.create()
  const session = await runtime.createSession('First conversation', fixture.directory)
  const path = runtime.pendingSessions.get(session.id).manager.getSessionFile()
  const persisted = await readFile(path, 'utf8')
  const route = sessionRuntimeRoutes.find(
    (item) => item.method === 'GET' && item.path === '/api/sessions/:sessionId/thinking-level',
  )
  assert.ok(route)
  const expected = {
    id: session.id,
    thinkingLevel: 'medium',
    availableLevels: [],
    status: 'unsupported',
    message: 'No model is configured for this session.',
    model: '',
  }
  const read = async (instance) => {
    let result
    await route.handler({
      runtime: instance,
      params: { sessionId: session.id },
      json: (status, data) => {
        result = { status, data }
      },
    })
    assert.deepEqual(result, { status: 200, data: expected })
    assert.equal(instance.sessions.size, 0)
    assert.deepEqual(instance.settingsManager.getGlobalSettings(), {
      defaultThinkingLevel: 'medium',
    })
    assert.equal(await readFile(path, 'utf8'), persisted)
  }
  await read(runtime)
  await assert.rejects(runtime.setSessionThinkingLevel(session.id, 'high'), /没有可用模型/)
  await fixture.close(runtime)
  const restarted = fixture.create()
  assert.equal(restarted.pendingSessions.size, 0)
  await read(restarted)
  assert.equal(await restarted.getSessionThinkingState('missing-session'), null)
})

test('a bound but unavailable model keeps its configuration error instead of appearing unconfigured', async (t) => {
  const fixture = await emptyConfigurationFixture(t)
  const runtime = fixture.create()
  const session = await runtime.createSession('Bound conversation', fixture.directory)
  runtime.pendingSessions.get(session.id).manager.appendModelChange('fixture', 'removed-model')
  const unavailable = new Error('The selected model is unavailable.')
  runtime.providerPreferences.resolveSessionModel = async (provider, modelId, options) => {
    assert.equal(provider, 'fixture')
    assert.equal(modelId, 'removed-model')
    assert.deepEqual(options, { requireEnabled: false })
    throw unavailable
  }
  await assert.rejects(
    runtime.getSessionThinkingState(session.id),
    (error) => error === unavailable,
  )
  assert.equal(runtime.sessions.size, 0)
})

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

test('thinking state clamps a stale persisted level to the active model levels', async () => {
  const session = {
    sessionId: 'session-k3-stale-level',
    thinkingLevel: 'medium',
    model: { provider: 'kimi-coding', id: 'k3' },
    getAvailableThinkingLevels: () => ['max'],
  }

  const state = await ProviderPreferences.prototype.getSessionThinkingState.call(
    preferencesFor(session),
    session.sessionId,
  )

  assert.equal(state.thinkingLevel, 'max')
  assert.deepEqual(state.availableLevels, ['max'])
  assert.equal(state.status, 'supported')
})

test('clampThinkingLevelToAvailable mirrors nearest-level semantics', () => {
  assert.equal(clampThinkingLevelToAvailable(['max'], 'medium'), 'max')
  assert.equal(clampThinkingLevelToAvailable(['off'], 'high'), 'off')
  assert.equal(clampThinkingLevelToAvailable(['low', 'high'], 'medium'), 'high')
  assert.equal(clampThinkingLevelToAvailable(['low', 'high'], 'off'), 'low')
  assert.equal(clampThinkingLevelToAvailable(['off', 'max'], 'max'), 'max')
  assert.equal(clampThinkingLevelToAvailable([], 'medium'), 'medium')
  assert.equal(clampThinkingLevelToAvailable(['high'], 'unknown'), 'high')
})

test('reconcileSessionThinkingLevel persists the clamped level onto the session', () => {
  const session = {
    thinkingLevel: 'medium',
    getAvailableThinkingLevels: () => ['max'],
    setThinkingLevel(level) {
      this.thinkingLevel = level
    },
  }
  const result = reconcileSessionThinkingLevel(session)
  assert.equal(session.thinkingLevel, 'max')
  assert.deepEqual(result, { availableLevels: ['max'], thinkingLevel: 'max', changed: true })

  const unchanged = reconcileSessionThinkingLevel(session)
  assert.equal(unchanged.changed, false)
})

test('reconcileSessionThinkingLevel leaves sessions without configurable levels untouched', () => {
  const session = {
    thinkingLevel: 'medium',
    getAvailableThinkingLevels: () => [],
    setThinkingLevel() {
      throw new Error('must not be called')
    },
  }
  const result = reconcileSessionThinkingLevel(session)
  assert.equal(session.thinkingLevel, 'medium')
  assert.equal(result.changed, false)
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
