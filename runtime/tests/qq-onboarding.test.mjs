import assert from 'node:assert/strict'
import test from 'node:test'
import { QQOnboardingService } from '../services/channels/qq-onboarding.mjs'

test('QQ onboarding displays connector QR and persists only the resulting official credentials', async () => {
  let completed
  let receivedOptions
  const service = new QQOnboardingService({
    onCompleted: async (credentials) => {
      completed = credentials
    },
    renderQr: async (url) => `qr:${url}`,
    startQrConnectImpl: (callbacks, options) => {
      receivedOptions = options
      callbacks.onQrDisplayed('https://qq.example/bind/task')
      setTimeout(
        () =>
          callbacks.onSuccess([
            { appId: 'app-123', appSecret: 'private-secret', userOpenid: 'owner-123' },
          ]),
        0,
      )
      return () => {}
    },
  })

  const job = await service.start()
  assert.equal(job.platform, 'qq')
  assert.equal(job.status, 'waiting')
  assert.equal(job.qrUrl, 'https://qq.example/bind/task')
  assert.equal(job.qrDataUrl, 'qr:https://qq.example/bind/task')
  assert.equal(job.appSecret, undefined)
  assert.equal(receivedOptions.displayQrCodeToConsole, false)
  assert.equal(receivedOptions.source, 'pisper')

  await new Promise((resolve) => setTimeout(resolve, 20))
  assert.deepEqual(completed, {
    appId: 'app-123',
    appSecret: 'private-secret',
    ownerUserId: 'owner-123',
  })
  assert.equal(service.get(job.id).status, 'completed')
})

test('QQ onboarding cancellation stops the connector and marks the job cancelled', async () => {
  let stopped = false
  const service = new QQOnboardingService({
    onCompleted: async () => {},
    startQrConnectImpl: (callbacks) => {
      callbacks.onQrDisplayed('https://qq.example/bind/task')
      return () => {
        stopped = true
      }
    },
  })

  const job = await service.start()
  assert.equal(service.cancel(job.id), true)
  assert.equal(stopped, true)
  assert.equal(service.get(job.id).status, 'cancelled')
})
