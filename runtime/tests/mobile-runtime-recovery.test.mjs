import assert from 'node:assert/strict'
import test from 'node:test'

import { createMobileRuntimeRecoveryCoordinator } from '../../src/lib/mobile-runtime-recovery.ts'

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
