import assert from 'node:assert/strict'
import test from 'node:test'
import {
  COMPACTION_SUMMARY_RESERVE_TOKENS,
  createCompactionSettingsManager,
  effectiveCompactionSettings,
  pisperCompactionExtension,
} from '../runtime/compaction-policy.mjs'

test('large context windows compact earlier without penalizing smaller models', () => {
  const base = { enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 }
  assert.deepEqual(effectiveCompactionSettings(base, 272_000), {
    ...base,
    reserveTokens: 54_400,
  })
  assert.equal(effectiveCompactionSettings(base, 128_000).reserveTokens, 25_600)
  assert.equal(effectiveCompactionSettings(base, 64_000).reserveTokens, 16_384)
  assert.equal(effectiveCompactionSettings(base, 1_000_000).reserveTokens, 65_536)
  assert.equal(effectiveCompactionSettings({ ...base, enabled: false }, 272_000).enabled, false)
})

test('session settings manager exposes the adaptive threshold and preserves method bindings', () => {
  const manager = {
    marker: 'base',
    getCompactionSettings() {
      assert.equal(this, manager)
      return { enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 }
    },
    getMarker() { return this.marker },
  }
  const wrapped = createCompactionSettingsManager(manager, () => 200_000)
  assert.equal(wrapped.getCompactionSettings().reserveTokens, 40_000)
  assert.equal(wrapped.getMarker(), 'base')
})

test('Pisper compaction uses no reasoning and keeps the summary output budget bounded', async () => {
  let handler
  pisperCompactionExtension({ on(type, callback) { if (type === 'session_before_compact') handler = callback } }, {
    compactSession: async (...args) => {
      const [preparation, model, apiKey, headers, instructions, signal, thinkingLevel, streamFn, env] = args
      assert.equal(preparation.settings.reserveTokens, COMPACTION_SUMMARY_RESERVE_TOKENS)
      assert.equal(model.id, 'reasoning-model')
      assert.equal(apiKey, 'secret')
      assert.deepEqual(headers, { 'x-test': 'yes' })
      assert.equal(instructions, 'Preserve decisions')
      assert.equal(signal.aborted, false)
      assert.equal(thinkingLevel, 'off')
      assert.equal(streamFn, undefined)
      assert.deepEqual(env, { TEST_ENV: '1' })
      return { summary: 'compact', firstKeptEntryId: 'entry-2', tokensBefore: 210_000, details: {} }
    },
  })

  const result = await handler({
    preparation: {
      settings: { enabled: true, reserveTokens: 54_400, keepRecentTokens: 20_000 },
      firstKeptEntryId: 'entry-2',
      tokensBefore: 210_000,
    },
    customInstructions: 'Preserve decisions',
    signal: new AbortController().signal,
  }, {
    model: { provider: 'openai', id: 'reasoning-model' },
    modelRegistry: {
      async getApiKeyAndHeaders() {
        return { ok: true, apiKey: 'secret', headers: { 'x-test': 'yes' }, env: { TEST_ENV: '1' } }
      },
    },
  })

  assert.equal(result.compaction.summary, 'compact')
})

test('Pisper compaction falls back to the SDK when extension auth is unavailable', async () => {
  let handler
  pisperCompactionExtension({ on(type, callback) { if (type === 'session_before_compact') handler = callback } })
  const result = await handler({ preparation: { settings: {} } }, {
    model: { provider: 'missing', id: 'model' },
    modelRegistry: { async getApiKeyAndHeaders() { return { ok: false, error: 'missing' } } },
  })
  assert.equal(result, undefined)
})
