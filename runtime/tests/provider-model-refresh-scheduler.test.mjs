import assert from 'node:assert/strict'
import test from 'node:test'
import {
  MODEL_LIST_REFRESH_INTERVAL_MS,
  MODEL_METADATA_MAX_AGE_MS,
  ProviderModelRefreshScheduler,
} from '../services/provider-model-refresh-scheduler.mjs'

function fakeTimers() {
  const timers = {
    callback: null,
    cleared: false,
    setInterval(callback) {
      timers.callback = callback
      return { unref() {} }
    },
    clearInterval() {
      timers.cleared = true
    },
  }
  return timers
}

function createScheduler(overrides = {}) {
  const state = { now: 1_000, busy: false, providerRefreshes: 0, metadataAges: [] }
  const timers = fakeTimers()
  const scheduler = new ProviderModelRefreshScheduler({
    isBusy: () => state.busy,
    refreshProviderModels: async () => {
      state.providerRefreshes += 1
    },
    refreshModelMetadata: async (maxAgeMs) => {
      state.metadataAges.push(maxAgeMs)
    },
    now: () => state.now,
    timers,
    ...overrides,
  })
  return { scheduler, state, timers }
}

test('model list refresh waits one interval after startup and runs only when idle', async () => {
  const { scheduler, state, timers } = createScheduler()
  scheduler.start()
  assert.equal(typeof timers.callback, 'function')

  await scheduler.tick()
  assert.equal(state.providerRefreshes, 0, 'startup already refreshed once')
  assert.deepEqual(state.metadataAges, [MODEL_METADATA_MAX_AGE_MS])

  state.now += MODEL_LIST_REFRESH_INTERVAL_MS
  state.busy = true
  assert.equal(scheduler.tick(), null)
  assert.equal(state.providerRefreshes, 0)
  assert.equal(state.metadataAges.length, 1, 'busy ticks skip metadata too')

  state.busy = false
  await scheduler.tick()
  assert.equal(state.providerRefreshes, 1)

  await scheduler.tick()
  assert.equal(state.providerRefreshes, 1, 'next refresh waits another interval')

  await scheduler.dispose()
  assert.equal(timers.cleared, true)
  assert.equal(scheduler.tick(), null)
})

test('model list refresh rechecks busy after metadata and coalesces overlapping ticks', async () => {
  let releaseMetadata
  const { scheduler, state } = createScheduler({
    refreshModelMetadata: () =>
      new Promise((resolve) => {
        releaseMetadata = resolve
      }),
  })
  scheduler.start()
  state.now += MODEL_LIST_REFRESH_INTERVAL_MS
  const first = scheduler.tick()
  assert.equal(scheduler.tick(), first)
  // 抓元数据期间有会话开跑：不动模型目录，下次空闲再来。
  state.busy = true
  releaseMetadata()
  await first
  assert.equal(state.providerRefreshes, 0)

  state.busy = false
  const second = scheduler.tick()
  releaseMetadata()
  await second
  assert.equal(state.providerRefreshes, 1)
})

test('model list refresh survives failures and an isBusy that throws', async (t) => {
  const warn = t.mock.method(console, 'warn', () => {})
  const { scheduler, state } = createScheduler({
    refreshModelMetadata: async () => {
      throw new Error('offline')
    },
    refreshProviderModels: async () => {
      state.providerRefreshes += 1
      throw new Error('relay down')
    },
  })
  scheduler.start()
  state.now += MODEL_LIST_REFRESH_INTERVAL_MS
  await scheduler.tick()
  assert.equal(state.providerRefreshes, 1)
  assert.equal(warn.mock.callCount(), 2)

  scheduler.isBusy = () => {
    throw new Error('broken')
  }
  state.now += MODEL_LIST_REFRESH_INTERVAL_MS
  assert.equal(scheduler.tick(), null, 'unknown state counts as busy')
})
