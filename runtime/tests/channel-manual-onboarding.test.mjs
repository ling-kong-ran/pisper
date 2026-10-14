import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { ChannelService } from '../services/channels/channel-service.mjs'

function fakeGateway() {
  return {
    getStatus: () => ({ state: 'idle', lastError: '', connectedAt: null, bot: null }),
    connect: async () => ({ state: 'connected' }),
    disconnect: async () => {},
    send: async () => {},
    sendToPeer: async () => {},
    sendAsset: async () => {},
    downloadResources: async () => [],
  }
}

test('QQ onboarding uses QR binding while Telegram keeps the manual BotFather form', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-manual-channels-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new ChannelService({
    path: join(directory, 'channels.json'),
    cwd: directory,
    agent: {
      prompt: async () => ({ sessionId: '', text: '', cwd: directory, model: '', assets: [] }),
      abort: async () => false,
      validateDirectory: async (value) => value,
    },
    gatewayFactories: {
      feishu: () => fakeGateway(),
      weixin: () => fakeGateway(),
      qq: () => fakeGateway(),
      telegram: () => fakeGateway(),
    },
    onboardingFactories: {
      qq: () => ({
        start: async () => ({
          mode: 'qr',
          platform: 'qq',
          id: 'qq-job',
          status: 'waiting',
          qrDataUrl: 'data:image/png;base64,qq',
        }),
        get: () => null,
        cancel: () => false,
        dispose: () => {},
      }),
    },
  })
  await service.init()
  const qqOnboarding = await service.startOnboarding('qq')
  assert.equal(qqOnboarding.mode, 'qr')
  assert.equal(qqOnboarding.platform, 'qq')
  assert.equal(qqOnboarding.status, 'waiting')
  assert.match(qqOnboarding.qrDataUrl, /^data:image\/png;base64,/)

  const telegramOnboarding = await service.startOnboarding('telegram')
  assert.equal(telegramOnboarding.mode, 'manual')
  assert.equal(telegramOnboarding.platform, 'telegram')
  assert.deepEqual(telegramOnboarding.fields, ['token'])
  assert.deepEqual(telegramOnboarding.required, ['token'])
  assert.equal(telegramOnboarding.setupUrl, 'https://t.me/BotFather')
  assert.match(telegramOnboarding.qrDataUrl, /^data:image\/png;base64,/)
  assert.equal(service.getState().connections.qq, null)
  assert.equal(service.getState().connections.telegram, null)
})
