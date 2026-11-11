import assert from 'node:assert/strict'
import test from 'node:test'
import {
  COMPACTION_SUMMARY_RESERVE_TOKENS,
  DEFAULT_COMPACTION_THRESHOLD_PERCENT,
  createCompactionSettingsManager,
  dedupeKeptReads,
  effectiveCompactionSettings,
  installTurnBoundaryCompaction,
  normalizeCompactionThresholdPercent,
  pisperCompactionExtension,
} from '../runtime/compaction-policy.mjs'

test('context windows use the configured percentage with a 90% default', () => {
  const base = { enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 }
  assert.equal(DEFAULT_COMPACTION_THRESHOLD_PERCENT, 90)
  assert.deepEqual(effectiveCompactionSettings(base, 272_000), {
    ...base,
    reserveTokens: 27_200,
  })
  assert.equal(effectiveCompactionSettings(base, 128_000).reserveTokens, 12_800)
  assert.equal(effectiveCompactionSettings(base, 64_000).reserveTokens, 6_400)
  assert.equal(effectiveCompactionSettings(base, 1_000_000).reserveTokens, 100_000)
  assert.equal(effectiveCompactionSettings(base, 200_000, 75).reserveTokens, 50_000)
  assert.equal(effectiveCompactionSettings({ ...base, enabled: false }, 272_000).enabled, false)
  assert.equal(normalizeCompactionThresholdPercent(9), 10)
  assert.equal(normalizeCompactionThresholdPercent(49), 49)
  assert.equal(normalizeCompactionThresholdPercent(96), 95)
  assert.equal(normalizeCompactionThresholdPercent('invalid'), 90)
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
        settings: { enabled: true, reserveTokens: 27_200, keepRecentTokens: 20_000 },
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

// ---- 压缩保留段的读取去重 ----
const COMPACTED_AT = 10_000

function readCall(id, args, timestamp) {
  return {
    role: 'assistant',
    timestamp,
    content: [{ type: 'toolCall', id, name: 'read', arguments: args }],
  }
}

function readResult(id, text, timestamp, extra = {}) {
  return {
    role: 'toolResult',
    toolCallId: id,
    toolName: 'read',
    isError: false,
    timestamp,
    content: [{ type: 'text', text }],
    ...extra,
  }
}

function summary() {
  return { role: 'compactionSummary', summary: 'earlier work', timestamp: COMPACTED_AT }
}

function keptConversation() {
  return [
    { role: 'user', content: 'old request', timestamp: 1_000 },
    summary(),
    readCall('a', { path: 'src/app.ts' }, 2_000),
    readResult('a', 'v1 of app', 2_001),
    readCall('b', { path: './src/app.ts' }, 3_000),
    readResult('b', 'v2 of app', 3_001),
  ]
}

test('kept reads superseded by a later full read become a placeholder', () => {
  const messages = keptConversation()
  const result = dedupeKeptReads(messages, { cwd: '/repo' })
  assert.notEqual(result, messages)
  assert.match(result[3].content[0].text, /^\[Earlier read of src\/app\.ts omitted/)
  assert.equal(result[5].content[0].text, 'v2 of app')
  // 不改原数组（会话文件里的消息保持原样）。
  assert.equal(messages[3].content[0].text, 'v1 of app')
  // 两次投影完全相同，前缀稳定。
  assert.deepEqual(dedupeKeptReads(keptConversation(), { cwd: '/repo' }), result)
})

test('messages after the compaction are never rewritten', () => {
  const messages = [
    summary(),
    readCall('a', { path: 'src/app.ts' }, 2_000),
    readResult('a', 'kept read', 2_001),
    { role: 'user', content: 'next', timestamp: COMPACTED_AT + 1 },
    readCall('b', { path: 'src/app.ts' }, COMPACTED_AT + 2),
    readResult('b', 'new read', COMPACTED_AT + 3),
    readCall('c', { path: 'src/app.ts' }, COMPACTED_AT + 4),
    readResult('c', 'newer read', COMPACTED_AT + 5),
  ]
  assert.equal(dedupeKeptReads(messages, { cwd: '/repo' }), messages)
})

test('partial or truncated later reads do not replace earlier ones', () => {
  const ranged = [
    summary(),
    readCall('a', { path: 'big.ts', offset: 1, limit: 100 }, 2_000),
    readResult('a', 'lines 1-100', 2_001),
    readCall('b', { path: 'big.ts', offset: 101, limit: 100 }, 3_000),
    readResult('b', 'lines 101-200', 3_001),
  ]
  assert.equal(dedupeKeptReads(ranged), ranged)

  const truncated = [
    summary(),
    readCall('a', { path: 'big.ts', offset: 3000 }, 2_000),
    readResult('a', 'tail of file', 2_001),
    readCall('b', { path: 'big.ts' }, 3_000),
    readResult('b', 'head\n\n[Showing lines 1-2000 of 4000. Use offset=2001 to continue.]', 3_001),
  ]
  assert.equal(dedupeKeptReads(truncated), truncated)

  const sameRange = [
    summary(),
    readCall('a', { path: 'big.ts', offset: 50, limit: 20 }, 2_000),
    readResult('a', 'old 50-69', 2_001),
    readCall('b', { path: 'big.ts', offset: 50, limit: 20 }, 3_000),
    readResult('b', 'new 50-69', 3_001),
  ]
  assert.match(dedupeKeptReads(sameRange)[2].content[0].text, /^\[Earlier read of big\.ts/)
})

test('failed reads, images and other files are left alone', () => {
  const messages = [
    summary(),
    readCall('a', { path: 'a.ts' }, 2_000),
    readResult('a', 'a.ts content', 2_001),
    readCall('b', { path: 'b.ts' }, 3_000),
    readResult('b', 'b.ts content', 3_001),
    readCall('c', { path: 'a.ts' }, 4_000),
    readResult('c', 'ENOENT', 4_001, { isError: true }),
    readCall('d', { path: 'shot.png' }, 5_000),
    {
      ...readResult('d', '', 5_001),
      content: [{ type: 'image', data: 'x', mimeType: 'image/png' }],
    },
    readCall('e', { path: 'shot.png' }, 6_000),
    {
      ...readResult('e', '', 6_001),
      content: [{ type: 'image', data: 'y', mimeType: 'image/png' }],
    },
  ]
  assert.equal(dedupeKeptReads(messages), messages)
})

test('conversations without a compaction pass through unchanged', () => {
  const messages = keptConversation().filter((message) => message.role !== 'compactionSummary')
  assert.equal(dedupeKeptReads(messages, { cwd: '/repo' }), messages)
})

test('the compaction extension projects deduped reads through the context hook', async () => {
  const handlers = new Map()
  pisperCompactionExtension({ on: (name, handler) => handlers.set(name, handler) })
  const contextHandler = handlers.get('context')
  assert.equal(typeof contextHandler, 'function')
  const deduped = await contextHandler({ messages: keptConversation() }, { cwd: '/repo' })
  assert.match(deduped.messages[3].content[0].text, /^\[Earlier read/)
  const untouched = await contextHandler(
    { messages: [{ role: 'user', content: 'hi', timestamp: 1 }] },
    { cwd: '/repo' },
  )
  assert.equal(untouched, undefined)
})
