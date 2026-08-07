import assert from 'node:assert/strict'
import test from 'node:test'
import { newerVersion, normalizedVersion } from '../../shared/app-update.mjs'
import {
  DESKTOP_UPDATE_INITIAL_DELAY_MS,
  DESKTOP_UPDATE_INTERVAL_MS,
  scheduleDesktopUpdateChecks,
  shouldAutomaticallyCheckForUpdates,
} from '../../src/features/updates/auto-update.ts'
import { checkWebUpdates } from '../../src/features/updates/update-client.ts'

test('web update versions normalize tags and compare semantic parts', () => {
  assert.equal(normalizedVersion('v1.2.3-beta.1'), '1.2.3')
  assert.equal(newerVersion('0.1.3', '0.1.2'), true)
  assert.equal(newerVersion('0.1.2', '0.1.2'), false)
  assert.equal(newerVersion('0.1.1', '0.1.2'), false)
})

test('web update checks use the same-origin uncached API', async () => {
  const result = await checkWebUpdates({
    refresh: true,
    fetcher: async (url, options) => {
      assert.equal(url, '/api/app-update?refresh=1')
      assert.equal(options.cache, 'no-store')
      return {
        ok: true,
        json: async () => ({
          state: 'available',
          currentVersion: '0.1.2',
          currentCommit: '1111111',
          availableCommit: '2222222',
          behindBy: 1,
          canDownload: false,
          message: 'Web 源码落后 main 1 个提交，请查看更新内容后自行更新。',
        }),
      }
    },
  })

  assert.equal(result.state, 'available')
  assert.equal(result.availableCommit, '2222222')
  assert.equal(result.canDownload, false)
  assert.match(result.message, /1 个提交/)
})

test('web update checks preserve the runtime error detail', async () => {
  await assert.rejects(
    checkWebUpdates({
      fetcher: async () => ({
        ok: false,
        status: 502,
        json: async () => ({ error: 'GitHub commit 比较失败：HTTP 403' }),
      }),
    }),
    /GitHub commit 比较失败：HTTP 403/,
  )
})

test('desktop update checks run after a delay and then periodically', async () => {
  let delayedCheck = null
  let periodicCheck = null
  const cleared = []
  const scheduler = {
    setTimeout(callback, delay) {
      assert.equal(delay, DESKTOP_UPDATE_INITIAL_DELAY_MS)
      delayedCheck = callback
      return 1
    },
    clearTimeout(handle) {
      cleared.push(['timeout', handle])
    },
    setInterval(callback, delay) {
      assert.equal(delay, DESKTOP_UPDATE_INTERVAL_MS)
      periodicCheck = callback
      return 2
    },
    clearInterval(handle) {
      cleared.push(['interval', handle])
    },
  }
  let checks = 0
  let finishCheck
  const stop = scheduleDesktopUpdateChecks(() => {
    checks += 1
    return new Promise((resolve) => {
      finishCheck = resolve
    })
  }, scheduler)

  delayedCheck()
  await Promise.resolve()
  assert.equal(checks, 1)
  periodicCheck()
  await Promise.resolve()
  assert.equal(checks, 1, 'overlapping automatic checks should be ignored')

  finishCheck()
  await new Promise((resolve) => setImmediate(resolve))
  periodicCheck()
  await Promise.resolve()
  assert.equal(checks, 2)

  stop()
  finishCheck()
  periodicCheck()
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(checks, 2)
  assert.deepEqual(cleared, [
    ['timeout', 1],
    ['interval', 2],
  ])
})

test('desktop automatic checks do not interrupt active or downloaded updates', () => {
  for (const state of ['checking', 'downloading', 'downloaded']) {
    assert.equal(shouldAutomaticallyCheckForUpdates(state), false)
  }
  for (const state of ['idle', 'current', 'available', 'error']) {
    assert.equal(shouldAutomaticallyCheckForUpdates(state), true)
  }
})
