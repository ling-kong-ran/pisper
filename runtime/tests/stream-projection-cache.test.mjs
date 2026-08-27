import assert from 'node:assert/strict'
import test from 'node:test'
import {
  ProjectionCache,
  addSessionUsage,
  emptySessionUsage,
  summarizeSessionUsage,
} from '../runtime/stream-projection.mjs'

test('session usage aggregates provider-reported cache and token fields', () => {
  const usage = summarizeSessionUsage([
    { role: 'user', usage: { input: 999, totalTokens: 999 } },
    {
      role: 'assistant',
      usage: {
        input: 100,
        output: 25,
        cacheRead: 300,
        cacheWrite: 100,
        reasoning: 10,
        totalTokens: 535,
      },
    },
    {
      role: 'assistant',
      usage: {
        input: 50,
        output: 20,
        cacheRead: 450,
        cacheWrite: 0,
        reasoning: 5,
        totalTokens: 525,
      },
    },
  ])

  assert.deepEqual(usage, {
    input: 150,
    output: 45,
    cacheRead: 750,
    cacheWrite: 100,
    reasoning: 15,
    totalTokens: 1060,
    processedTokens: 295,
    requests: 2,
    promptTokens: 1000,
    cacheReported: true,
    cacheHitRate: 75,
  })
})

test('session usage keeps the cache hit rate unknown when the provider omits cache fields', () => {
  // 部分中转只回传 input/output（实测：{"input_tokens":12313,"output_tokens":4}），
  // 此时命中率不可知，不能与真实 0% 命中混为一谈。
  const omitted = emptySessionUsage()
  addSessionUsage(omitted, { input: 12313, output: 4, totalTokens: 12317 })
  assert.equal(omitted.cacheReported, false)
  assert.equal(omitted.cacheHitRate, null)
  assert.equal(omitted.cacheRead, 0)

  // 上游上报了字段但确实未命中：应得到真实的 0%。
  const missed = emptySessionUsage()
  addSessionUsage(missed, { input: 500, output: 10, cacheRead: 0, cacheWrite: 0 })
  assert.equal(missed.cacheReported, true)
  assert.equal(missed.cacheHitRate, 0)

  // 同一会话中只要有任何一次请求上报过缓存，后续就能继续统计。
  addSessionUsage(omitted, { input: 100, output: 5, cacheRead: 900 })
  assert.equal(omitted.cacheReported, true)
  assert.equal(omitted.cacheHitRate, (900 / 13313) * 100)
})

test('session usage falls back to field totals only when provider total is absent', () => {
  const usage = emptySessionUsage()
  addSessionUsage(usage, {
    input: 10,
    output: 4,
    cacheRead: 20,
    cacheWrite: 2,
    reasoning: 3,
  })
  addSessionUsage(usage, {
    input: 1,
    output: 2,
    cacheRead: 0,
    cacheWrite: 0,
    reasoning: 99,
    totalTokens: 8,
  })

  assert.equal(usage.totalTokens, 44)
  assert.equal(usage.processedTokens, 19)
  assert.equal(usage.promptTokens, 33)
  assert.equal(usage.cacheHitRate, (20 / 33) * 100)
  assert.equal(usage.requests, 2)
})

test('transcript projection preserves identity until its mutable source changes', () => {
  const cache = new ProjectionCache()
  const messages = [{ role: 'assistant', content: 'first' }]
  let builds = 0
  const project = () => cache.transcript('session-1', messages, () => ({ build: ++builds }))

  const first = project()
  assert.equal(project(), first)
  assert.equal(builds, 1)

  messages[0].content = 'streamed update'
  const streamed = project()
  assert.notEqual(streamed, first)
  assert.equal(builds, 2)

  messages.push({ role: 'toolResult', content: 'done' })
  const appended = project()
  assert.notEqual(appended, streamed)
  assert.equal(builds, 3)
  assert.equal(project(), appended)
})

test('projection caches evict least-recent sessions and skip oversized values', () => {
  const cache = new ProjectionCache({ maxEntries: 2, maxBytes: 256 })
  const messages = (content) => [{ role: 'assistant', content }]

  cache.transcript('session-1', messages('one'), () => ({ text: 'one' }))
  cache.transcript('session-2', messages('two'), () => ({ text: 'two' }))
  cache.transcript('session-3', messages('three'), () => ({ text: 'three' }))
  assert.equal(cache.transcripts.size, 2)
  assert.equal(cache.transcripts.has('session-1'), false)

  const oversizedMessages = messages('large')
  let builds = 0
  const build = () => ({ text: 'x'.repeat(200), build: ++builds })
  cache.transcript('session-large', oversizedMessages, build)
  cache.transcript('session-large', oversizedMessages, build)
  assert.equal(builds, 2)
  assert.equal(cache.transcripts.has('session-large'), false)

  const stats = cache.stats()
  assert.equal(stats.maxEntries, 2)
  assert.equal(stats.maxEstimatedBytes, 256)
  assert.ok(stats.transcripts.estimatedBytes <= 256)
})

test('projection invalidation clears only the requested session scopes', () => {
  const cache = new ProjectionCache()
  const messages = [{ role: 'user', content: 'hello' }]
  const transcript = cache.transcript('session-1', messages, () => ['transcript'])
  const withAssets = cache.transcriptWithAssets('session-1', transcript, 1, () => ['assets'])
  const usageToken = [messages, messages.length]
  const usage = cache.contextUsage('session-1', usageToken, () => ({ tokens: 1 }))
  const liveToken = [messages, false]
  const live = cache.storeLiveSnapshot('session-1', liveToken, { streaming: false })

  cache.invalidate('session-1', { transcript: false, activity: true, usage: false })

  assert.equal(
    cache.transcript('session-1', messages, () => ['rebuilt']),
    transcript,
  )
  assert.equal(
    cache.transcriptWithAssets('session-1', transcript, 1, () => ['rebuilt']),
    withAssets,
  )
  assert.equal(
    cache.contextUsage('session-1', usageToken, () => ({ tokens: 2 })),
    usage,
  )
  assert.deepEqual(cache.liveSnapshot('session-1', liveToken), { hit: false, value: null })
  assert.equal(live.streaming, false)

  cache.invalidate('session-1')
  assert.notEqual(
    cache.transcript('session-1', messages, () => ['rebuilt']),
    transcript,
  )
  assert.notEqual(
    cache.contextUsage('session-1', usageToken, () => ({ tokens: 2 })),
    usage,
  )
})

test('asset, usage, and session invalidation evict dependent live snapshots', () => {
  const cache = new ProjectionCache()
  const transcript = ['message']
  const liveToken = ['live']

  const assets = cache.transcriptWithAssets('session-1', transcript, 1, () => ['asset-1'])
  cache.storeLiveSnapshot('session-1', liveToken, { revision: 1 })
  cache.invalidateAssets()
  assert.notEqual(
    cache.transcriptWithAssets('session-1', transcript, 1, () => ['asset-2']),
    assets,
  )
  assert.equal(cache.liveSnapshot('session-1', liveToken).hit, false)

  const usageToken = ['usage']
  const usage = cache.contextUsage('session-1', usageToken, () => ({ tokens: 1 }))
  cache.storeLiveSnapshot('session-1', liveToken, { revision: 2 })
  cache.invalidateAllUsage()
  assert.notEqual(
    cache.contextUsage('session-1', usageToken, () => ({ tokens: 2 })),
    usage,
  )
  assert.equal(cache.liveSnapshot('session-1', liveToken).hit, false)

  cache.storeLiveSnapshot('session-1', liveToken, { revision: 3 })
  cache.delete('session-1')
  assert.equal(cache.liveSnapshot('session-1', liveToken).hit, false)
})
