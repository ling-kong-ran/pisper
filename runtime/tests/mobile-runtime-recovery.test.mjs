import assert from 'node:assert/strict'
import test from 'node:test'

import { createMobileRuntimeRecoveryCoordinator } from '../../src/lib/mobile-runtime-recovery.ts'
import { createRemoteFallback } from '../../src/lib/mobile-remote-fallback.ts'

test('mobile foreground recovery gates API work behind one shared readiness check', async () => {
  let releaseResume
  let resumeCalls = 0
  let probeCalls = 0
  const resumePending = new Promise((resolve) => {
    releaseResume = resolve
  })
  const recovery = createMobileRuntimeRecoveryCoordinator({
    isMobile: () => true,
    resume: async () => {
      resumeCalls += 1
      await resumePending
    },
    probe: async () => {
      probeCalls += 1
      return true
    },
    reload: () => new Promise(() => undefined),
  })

  recovery.markBackgrounded()
  const foreground = recovery.recoverAfterForeground()
  const requestGate = recovery.waitUntilReady()

  assert.strictEqual(requestGate, foreground)
  assert.equal(resumeCalls, 1)
  assert.equal(probeCalls, 0)
  releaseResume()
  await Promise.all([foreground, requestGate])
  assert.equal(probeCalls, 1)

  await recovery.waitUntilReady()
  assert.equal(resumeCalls, 1)
})

test('failed mobile foreground recovery remains retryable for the next API request', async () => {
  let resumeCalls = 0
  const recovery = createMobileRuntimeRecoveryCoordinator({
    isMobile: () => true,
    resume: async () => {
      resumeCalls += 1
      if (resumeCalls === 1) throw new Error('runtime still thawing')
    },
    probe: async () => true,
    reload: () => new Promise(() => undefined),
  })

  recovery.markBackgrounded()
  await assert.rejects(recovery.recoverAfterForeground(), /runtime still thawing/)
  await recovery.waitUntilReady()
  assert.equal(resumeCalls, 2)
})

test('failed WebView loopback probe rebuilds the current page instead of releasing API calls', async () => {
  const reloadError = new Error('navigation started')
  let reloads = 0
  const recovery = createMobileRuntimeRecoveryCoordinator({
    isMobile: () => true,
    resume: async () => undefined,
    probe: async () => false,
    reload: async () => {
      reloads += 1
      throw reloadError
    },
  })

  recovery.markBackgrounded()
  await assert.rejects(recovery.waitUntilReady(), (error) => error === reloadError)
  assert.equal(reloads, 1)
})

// 回归：resume 原生调用挂起（如 iOS 冻结 socket）时，恢复尝试必须在超时后继续，
// 否则全部 API 请求会永远等在闸门后，界面停在「正在唤醒 Agent」。
test('a hung resume step times out and recovery still completes via the probe', async () => {
  let probeCalls = 0
  const recovery = createMobileRuntimeRecoveryCoordinator({
    isMobile: () => true,
    resume: () => new Promise(() => undefined),
    probe: async () => {
      probeCalls += 1
      return true
    },
    reload: () => new Promise(() => undefined),
    timeouts: { resumeMs: 10, probeMs: 10, reloadMs: 10 },
  })

  recovery.markBackgrounded()
  await recovery.waitUntilReady()
  assert.equal(probeCalls, 1)
  // 恢复已收敛：后续请求不再被闸门拦截。
  await recovery.waitUntilReady()
})

// 回归：探测 fetch 挂起视同不健康并触发页面重载，而不是永久等待。
test('a hung probe is treated as unhealthy and reloads the page', async () => {
  let reloads = 0
  const recovery = createMobileRuntimeRecoveryCoordinator({
    isMobile: () => true,
    resume: async () => undefined,
    probe: () => new Promise(() => undefined),
    reload: async () => {
      reloads += 1
      // 返回一个永不了结的 Promise，模拟导航后页面不再执行后续代码。
      await new Promise(() => undefined)
      return undefined
    },
    timeouts: { resumeMs: 10, probeMs: 10, reloadMs: 10 },
  })

  recovery.markBackgrounded()
  await recovery.waitUntilReady()
  assert.equal(reloads, 1)
})

// 回归：重载导航本身挂起时放弃重载并放行请求，不形成第二个永久等待点。
test('a hung reload gives up and releases the gate instead of pending forever', async () => {
  const recovery = createMobileRuntimeRecoveryCoordinator({
    isMobile: () => true,
    resume: async () => undefined,
    probe: async () => false,
    reload: () => new Promise(() => undefined),
    timeouts: { resumeMs: 10, probeMs: 10, reloadMs: 10 },
  })

  recovery.markBackgrounded()
  await recovery.waitUntilReady()
  await recovery.waitUntilReady()
})

// 远程回落：只有远程模式 + 不可达类错误才切回本机；业务错误与本机模式不触发。
test('remote unreachable errors switch the app back to local mode exactly once', async () => {
  const calls = []
  const fallback = createRemoteFallback({
    isMobileApp: () => true,
    getMode: async () => 'remote',
    enterLocal: async () => {
      calls.push('enterLocal')
    },
    markNotice: () => calls.push('markNotice'),
    reload: () => calls.push('reload'),
  })

  const timeoutError = new DOMException('Request timed out', 'TimeoutError')
  assert.equal(await fallback.handleFailure(timeoutError), true)
  assert.deepEqual(calls, ['enterLocal', 'markNotice', 'reload'])

  // 冷却窗口内的第二次失败只返回 false，不重复调用原生命令或刷新页面。
  assert.equal(await fallback.handleFailure({ status: 502 }), false)
  assert.deepEqual(calls, ['enterLocal', 'markNotice', 'reload'])
})

test('remote fallback ignores local mode, non-mobile pages, and business errors', async () => {
  const calls = []
  const fallback = createRemoteFallback({
    isMobileApp: () => true,
    getMode: async () => 'local',
    enterLocal: async () => {
      calls.push('enterLocal')
    },
    markNotice: () => calls.push('markNotice'),
    reload: () => calls.push('reload'),
  })
  assert.equal(await fallback.handleFailure(new DOMException('x', 'TimeoutError')), false)
  assert.deepEqual(calls, [])

  const remoteButBusinessError = createRemoteFallback({
    isMobileApp: () => true,
    getMode: async () => 'remote',
    enterLocal: async () => {
      calls.push('enterLocal')
    },
    markNotice: () => calls.push('markNotice'),
    reload: () => calls.push('reload'),
  })
  assert.equal(await remoteButBusinessError.handleFailure({ status: 400 }), false)
  assert.deepEqual(calls, [])

  const desktop = createRemoteFallback({
    isMobileApp: () => false,
    getMode: async () => 'remote',
    enterLocal: async () => {
      calls.push('enterLocal')
    },
    markNotice: () => calls.push('markNotice'),
    reload: () => calls.push('reload'),
  })
  assert.equal(await desktop.handleFailure(new TypeError('fetch failed')), false)
  assert.deepEqual(calls, [])
})

test('remote fallback accepts wrapped timeout/network errors but not cancellation', async () => {
  const calls = []
  const fallback = createRemoteFallback({
    isMobileApp: () => true,
    getMode: async () => 'remote',
    enterLocal: async () => calls.push('enterLocal'),
    markNotice: () => calls.push('markNotice'),
    reload: () => calls.push('reload'),
    cooldownMs: 0,
  })

  assert.equal(await fallback.handleFailure({ name: 'ApiError', kind: 'timeout' }), true)
  assert.deepEqual(calls, ['enterLocal', 'markNotice', 'reload'])

  const cancelled = createRemoteFallback({
    isMobileApp: () => true,
    getMode: async () => 'remote',
    enterLocal: async () => calls.push('enterLocal'),
    markNotice: () => calls.push('markNotice'),
    reload: () => calls.push('reload'),
  })
  assert.equal(await cancelled.handleFailure({ name: 'ApiError', kind: 'cancelled' }), false)
})

test('remote fallback has a bounded native transition and can retry after its cooldown', async () => {
  let now = 1_000
  let enterLocalCalls = 0
  const fallback = createRemoteFallback({
    isMobileApp: () => true,
    getMode: async () => 'remote',
    enterLocal: () => {
      enterLocalCalls += 1
      return new Promise(() => undefined)
    },
    markNotice: () => undefined,
    reload: () => undefined,
    now: () => now,
    cooldownMs: 100,
    timeouts: { stateMs: 10, enterLocalMs: 10 },
  })

  // 让第一次调用进入超时路径，并确认它不会挂起测试/界面。
  assert.equal(await fallback.handleFailure({ status: 502 }), false)
  now += 101
  assert.equal(await fallback.handleFailure({ status: 502 }), false)
  // 永不返回的 enterLocal 被两次有界等待接住，而不是留下 pending。
  assert.equal(enterLocalCalls, 2)
})

test('remote fallback keeps the original error visible when entering local mode fails', async () => {
  const calls = []
  const fallback = createRemoteFallback({
    isMobileApp: () => true,
    getMode: async () => 'remote',
    enterLocal: async () => {
      throw new Error('on-device runtime unavailable')
    },
    markNotice: () => calls.push('markNotice'),
    reload: () => calls.push('reload'),
  })
  assert.equal(await fallback.handleFailure({ status: 502 }), false)
  assert.deepEqual(calls, [])
})
