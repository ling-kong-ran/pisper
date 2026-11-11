import assert from 'node:assert/strict'
import test from 'node:test'
import {
  COMPACTION_SUMMARY_RESERVE_TOKENS,
  DEFAULT_COMPACTION_MAX_CONTEXT_TOKENS,
  DEFAULT_COMPACTION_THRESHOLD_PERCENT,
  createCompactionSettingsManager,
  effectiveCompactionSettings,
  installTurnBoundaryCompaction,
  normalizeCompactionMaxContextTokens,
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
  // 不设上限时纯按百分比；默认上限下 1M 窗口在 256K 就压缩。
  assert.equal(effectiveCompactionSettings(base, 1_000_000, 80, 0).reserveTokens, 200_000)
  assert.equal(effectiveCompactionSettings(base, 1_000_000).reserveTokens, 744_000)
  assert.equal(effectiveCompactionSettings(base, 200_000, 75).reserveTokens, 50_000)
  assert.equal(effectiveCompactionSettings({ ...base, enabled: false }, 272_000).enabled, false)
  assert.equal(normalizeCompactionThresholdPercent(9), 10)
  assert.equal(normalizeCompactionThresholdPercent(49), 49)
  assert.equal(normalizeCompactionThresholdPercent(96), 95)
  assert.equal(normalizeCompactionThresholdPercent('invalid'), 80)
})

test('the absolute cap lowers the trigger only on large windows', () => {
  const base = { enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 }
  assert.equal(DEFAULT_COMPACTION_MAX_CONTEXT_TOKENS, 256_000)
  const trigger = (window, percent, cap) =>
    window - effectiveCompactionSettings(base, window, percent, cap).reserveTokens
  // 200K 窗口按 80% 本来就在 160K 触发，上限不起作用。
  assert.equal(trigger(200_000, 80), 160_000)
  assert.equal(trigger(1_048_576, 80), 256_000)
  assert.equal(trigger(1_048_576, 80, 512_000), 512_000)
  assert.equal(trigger(1_048_576, 80, 0), Math.floor(1_048_576 * 0.8))
  // 保留尾部跟着触发线走，不会反过来超过它的一半。
  assert.equal(effectiveCompactionSettings(base, 1_048_576).keepRecentTokens, 20_000)
  assert.equal(
    effectiveCompactionSettings({ ...base, keepRecentTokens: 400_000 }, 1_048_576).keepRecentTokens,
    128_000,
  )
  assert.equal(normalizeCompactionMaxContextTokens(0), 0)
  assert.equal(normalizeCompactionMaxContextTokens('0'), 0)
  assert.equal(normalizeCompactionMaxContextTokens(512_000), 512_000)
  assert.equal(normalizeCompactionMaxContextTokens(5), 256_000)
  assert.equal(normalizeCompactionMaxContextTokens('junk'), 256_000)
  assert.equal(normalizeCompactionMaxContextTokens(undefined), 256_000)
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

test('retained context adapts to smaller windows and lower thresholds without enlarging a user limit', () => {
  const base = { enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 }
  for (const [window, threshold] of [
    [8_192, 80],
    [16_000, 80],
    [32_000, 50],
    [128_000, 10],
  ]) {
    const settings = effectiveCompactionSettings(base, window, threshold)
    const trigger = window - settings.reserveTokens
    assert.ok(settings.keepRecentTokens <= trigger / 2)
    assert.ok(settings.keepRecentTokens > 0)
  }
  assert.equal(
    effectiveCompactionSettings({ ...base, keepRecentTokens: 100 }, 16_000).keepRecentTokens,
    100,
  )
  assert.equal(effectiveCompactionSettings(base, 0).keepRecentTokens, 20_000)
  assert.equal(base.keepRecentTokens, 20_000)
  let window = 128_000
  let threshold = 80
  const wrapped = createCompactionSettingsManager(
    { getCompactionSettings: () => base },
    () => window,
    () => threshold,
  )
  assert.equal(wrapped.getCompactionSettings().keepRecentTokens, 20_000)
  window = 16_000
  const smallWindow = wrapped.getCompactionSettings().keepRecentTokens
  assert.ok(smallWindow < 20_000)
  threshold = 10
  assert.ok(wrapped.getCompactionSettings().keepRecentTokens < smallWindow)
})

test('summary output and the kept tail leave room below the next compaction trigger', async () => {
  let handler
  let preparation
  pisperCompactionExtension(
    {
      on: (_event, callback) => {
        handler = callback
      },
    },
    {
      compactSession: async (value) => {
        preparation = value
        return { summary: 'summary' }
      },
    },
  )
  for (const [contextWindow, threshold] of [
    [8_192, 80],
    [32_000, 50],
    [128_000, 10],
    [128_000, 80],
  ]) {
    const settings = effectiveCompactionSettings({}, contextWindow, threshold)
    const event = { preparation: { settings }, signal: new AbortController().signal }
    await handler(event, {
      model: { contextWindow },
      modelRegistry: { getApiKeyAndHeaders: async () => ({ ok: true, apiKey: 'test' }) },
    })
    const worstCaseSummary =
      Math.floor(preparation.settings.reserveTokens * 0.8) +
      Math.floor(preparation.settings.reserveTokens * 0.5)
    assert.ok(worstCaseSummary + settings.keepRecentTokens < contextWindow - settings.reserveTokens)
    assert.equal(event.preparation.settings, settings)
    assert.ok(preparation.settings.reserveTokens <= COMPACTION_SUMMARY_RESERVE_TOKENS)
  }
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
        assert.equal(model.maxTokens, Math.floor(COMPACTION_SUMMARY_RESERVE_TOKENS * 0.8))
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
