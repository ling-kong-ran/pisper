import assert from 'node:assert/strict'
import test from 'node:test'
import {
  COMPACTION_SUMMARY_RESERVE_TOKENS,
  DEFAULT_COMPACTION_THRESHOLD_PERCENT,
  createCompactionSettingsManager,
  effectiveCompactionSettings,
  installTurnBoundaryCompaction,
  normalizeCompactionThresholdPercent,
  pisperCompactionExtension,
} from '../runtime/compaction-policy.mjs'

test('context windows use the configured percentage with an 80% default', () => {
  const base = { enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 }
  assert.equal(DEFAULT_COMPACTION_THRESHOLD_PERCENT, 80)
  assert.deepEqual(effectiveCompactionSettings(base, 272_000), {
    ...base,
    reserveTokens: 54_400,
  })
  assert.equal(effectiveCompactionSettings(base, 128_000).reserveTokens, 25_600)
  assert.equal(effectiveCompactionSettings(base, 64_000).reserveTokens, 12_800)
  assert.equal(effectiveCompactionSettings(base, 1_000_000).reserveTokens, 200_000)
  assert.equal(effectiveCompactionSettings(base, 200_000, 75).reserveTokens, 50_000)
  assert.equal(effectiveCompactionSettings({ ...base, enabled: false }, 272_000).enabled, false)
  assert.equal(normalizeCompactionThresholdPercent(49), 50)
  assert.equal(normalizeCompactionThresholdPercent(96), 95)
  assert.equal(normalizeCompactionThresholdPercent('invalid'), 80)
})

test('session settings manager exposes the adaptive threshold and preserves method bindings', () => {
  const manager = {
    marker: 'base',
    getCompactionSettings() {
      assert.equal(this, manager)
      return { enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 }
    },
    getMarker() {
      return this.marker
    },
  }
  let threshold = 80
  const wrapped = createCompactionSettingsManager(
    manager,
    () => 200_000,
    () => threshold,
  )
  assert.equal(wrapped.getCompactionSettings().reserveTokens, 40_000)
  threshold = 75
  assert.equal(wrapped.getCompactionSettings().reserveTokens, 50_000)
  assert.equal(wrapped.getMarker(), 'base')
})

test('tool turns compact before the next provider request and resume with rebuilt context', async () => {
  const originalContext = { systemPrompt: 'current', messages: ['before'], tools: ['read'] }
  const refreshedContext = { ...originalContext, systemPrompt: 'refreshed' }
  const compactedMessages = ['summary', 'recent-tool-result']
  const calls = []
  let leaf = 'tool-result'
  const session = {
    agent: {
      prepareNextTurnWithContext: async (turn, signal) => {
        calls.push(['previous', turn.message.model, signal.aborted])
        return { context: refreshedContext, model: 'refreshed-model' }
      },
    },
    sessionManager: {
      getLeafId: () => leaf,
      buildSessionContext: () => ({ messages: compactedMessages }),
    },
    async _checkCompaction(message) {
      calls.push(['compact', message.model])
      leaf = 'compaction'
      return false
    },
  }

  installTurnBoundaryCompaction(session)
  installTurnBoundaryCompaction(session)
  const result = await session.agent.prepareNextTurnWithContext(
    {
      message: { model: 'test-model' },
      toolResults: [{ role: 'toolResult' }],
      context: originalContext,
    },
    new AbortController().signal,
  )

  assert.deepEqual(calls, [
    ['previous', 'test-model', false],
    ['compact', 'test-model'],
  ])
  assert.deepEqual(result, {
    context: { ...refreshedContext, messages: compactedMessages },
    model: 'refreshed-model',
  })
})

test('turn boundary compaction leaves ordinary assistant turns unchanged', async () => {
  let checks = 0
  const session = {
    agent: {},
    sessionManager: {
      getLeafId: () => 'assistant',
      buildSessionContext: () => {
        throw new Error('context should not be rebuilt')
      },
    },
    async _checkCompaction() {
      checks += 1
      return false
    },
  }
  installTurnBoundaryCompaction(session)

  assert.equal(
    await session.agent.prepareNextTurnWithContext(
      { message: { model: 'test-model' }, toolResults: [], context: { messages: [] } },
      new AbortController().signal,
    ),
    undefined,
  )
  assert.equal(checks, 0)
})

test('Pisper compaction uses no reasoning and keeps the summary output budget bounded', async () => {
  let handler
  pisperCompactionExtension(
    {
      on(type, callback) {
        if (type === 'session_before_compact') handler = callback
      },
    },
    {
      compactSession: async (...args) => {
        const [
          preparation,
          model,
          apiKey,
          headers,
          instructions,
          signal,
          thinkingLevel,
          streamFn,
          env,
        ] = args
        assert.equal(preparation.settings.reserveTokens, COMPACTION_SUMMARY_RESERVE_TOKENS)
        assert.equal(model.id, 'reasoning-model')
        assert.equal(apiKey, 'secret')
        assert.deepEqual(headers, { 'x-test': 'yes' })
        assert.equal(instructions, 'Preserve decisions')
        assert.equal(signal.aborted, false)
        assert.equal(thinkingLevel, 'off')
        assert.equal(streamFn, undefined)
        assert.deepEqual(env, { TEST_ENV: '1' })
        return {
          summary: 'compact',
          firstKeptEntryId: 'entry-2',
          tokensBefore: 210_000,
          details: {},
        }
      },
    },
  )

  const result = await handler(
    {
      preparation: {
        settings: { enabled: true, reserveTokens: 54_400, keepRecentTokens: 20_000 },
        firstKeptEntryId: 'entry-2',
        tokensBefore: 210_000,
      },
      customInstructions: 'Preserve decisions',
      signal: new AbortController().signal,
    },
    {
      model: { provider: 'openai', id: 'reasoning-model' },
      modelRegistry: {
        async getApiKeyAndHeaders() {
          return {
            ok: true,
            apiKey: 'secret',
            headers: { 'x-test': 'yes' },
            env: { TEST_ENV: '1' },
          }
        },
      },
    },
  )

  assert.equal(result.compaction.summary, 'compact')
})

test('Pisper compaction falls back to the SDK when extension auth is unavailable', async () => {
  let handler
  pisperCompactionExtension({
    on(type, callback) {
      if (type === 'session_before_compact') handler = callback
    },
  })
  const result = await handler(
    { preparation: { settings: {} } },
    {
      model: { provider: 'missing', id: 'model' },
      modelRegistry: {
        async getApiKeyAndHeaders() {
          return { ok: false, error: 'missing' }
        },
      },
    },
  )
  assert.equal(result, undefined)
})
