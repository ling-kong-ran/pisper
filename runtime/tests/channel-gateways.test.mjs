import assert from 'node:assert/strict'
import test from 'node:test'
import { QQBotApi } from '../services/channels/qq-gateway.mjs'
import { TelegramGateway } from '../services/channels/telegram-gateway.mjs'

function response(payload, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    async json() {
      return payload
    },
  }
}

test('Telegram gateway maps direct and group messages and preserves attachments', () => {
  const gateway = new TelegramGateway({ fetchImpl: async () => response({ ok: true, result: [] }) })
  assert.deepEqual(
    gateway.mapMessage({
      message_id: 12,
      chat: { id: -100, type: 'supergroup' },
      from: { id: 7, first_name: 'Ada' },
      caption: '请看图',
      photo: [{ file_id: 'small' }, { file_id: 'large' }],
    }),
    {
      messageId: '12',
      peerId: '-100',
      senderId: '7',
      senderName: 'Ada',
      chatType: 'group',
      content: '请看图',
      resources: [{ type: 'image', fileId: 'large', name: 'telegram-large.jpg' }],
    },
  )
})

test('Telegram gateway calls Bot API with the configured token', async () => {
  const calls = []
  const gateway = new TelegramGateway({
    fetchImpl: async (url, options) => {
      calls.push({ url, options })
      if (url.endsWith('/getUpdates'))
        return new Promise((_, reject) =>
          options.signal.addEventListener('abort', () => reject(new Error('aborted')), {
            once: true,
          }),
        )
      return response({
        ok: true,
        result: { id: 42, first_name: 'Pisper', username: 'pisper_bot' },
      })
    },
  })
  const status = await gateway.connect({ token: `12345:${'a'.repeat(20)}` })
  assert.equal(status.state, 'connected')
  assert.match(calls[0].url, /\/bot12345:.*\/getMe$/)
  await gateway.disconnect()
})

test('QQ Bot API exchanges credentials and sends a message through the OpenAPI', async () => {
  const calls = []
  const api = new QQBotApi({
    fetchImpl: async (url, options) => {
      calls.push({ url, options })
      if (url.includes('getAppAccessToken')) return response({ access_token: 'access-token' })
      if (url.endsWith('/gateway')) return response({ url: 'wss://qq.example/gateway' })
      if (url.endsWith('/users/@me')) return response({ id: 'bot-id', username: 'Pisper' })
      return response({ id: 'message-id' })
    },
  })
  await api.getAppAccessToken('app-id', 'app-secret')
  assert.equal(await api.getGateway(), 'wss://qq.example/gateway')
  await api.sendMessage({ peerId: 'user-openid', chatType: 'p2p' }, { content: '你好' })
  assert.equal(calls.at(-1).url, 'https://api.sgroup.qq.com/v2/users/user-openid/messages')
  assert.equal(JSON.parse(calls.at(-1).options.body).content, '你好')
})
