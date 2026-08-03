import assert from 'node:assert/strict'
import test from 'node:test'
import { ProjectionCache } from '../runtime/stream-projection.mjs'

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

  assert.equal(cache.transcript('session-1', messages, () => ['rebuilt']), transcript)
  assert.equal(
    cache.transcriptWithAssets('session-1', transcript, 1, () => ['rebuilt']),
    withAssets,
  )
  assert.equal(cache.contextUsage('session-1', usageToken, () => ({ tokens: 2 })), usage)
  assert.deepEqual(cache.liveSnapshot('session-1', liveToken), { hit: false, value: null })
  assert.equal(live.streaming, false)

  cache.invalidate('session-1')
  assert.notEqual(cache.transcript('session-1', messages, () => ['rebuilt']), transcript)
  assert.notEqual(cache.contextUsage('session-1', usageToken, () => ({ tokens: 2 })), usage)
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
  assert.notEqual(cache.contextUsage('session-1', usageToken, () => ({ tokens: 2 })), usage)
  assert.equal(cache.liveSnapshot('session-1', liveToken).hit, false)

  cache.storeLiveSnapshot('session-1', liveToken, { revision: 3 })
  cache.delete('session-1')
  assert.equal(cache.liveSnapshot('session-1', liveToken).hit, false)
})
