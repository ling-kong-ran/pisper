import assert from 'node:assert/strict'
import test from 'node:test'
import { FeishuGateway } from '../services/channels/feishu-gateway.mjs'
import { QQBotApi, QQGateway } from '../services/channels/qq-gateway.mjs'
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
      chat: { id: -100, type: 'supergroup', title: '发布群' },
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
      chatTitle: '发布群',
      content: '请看图',
      resources: [{ type: 'image', fileId: 'large', name: 'telegram-large.jpg' }],
    },
  )
})

test('Telegram gateway marks group messages addressed to the bot, whatever the form', () => {
  const gateway = new TelegramGateway({ fetchImpl: async () => response({ ok: true, result: [] }) })
  gateway.status.bot = { id: '99', name: 'Pisper', username: 'Pisper_Bot' }
  const group = (input) =>
    gateway.mapMessage({
      message_id: 1,
      chat: { id: -100, type: 'group' },
      from: { id: 7, first_name: 'Ada' },
      ...input,
    })
  const mention = (text, word) => [
    { type: 'mention', offset: text.indexOf(word), length: word.length },
  ]
  // 实体偏移按 UTF-16：前面有中文也能对上。
  const text = '喂 @pisper_bot 值班 2'
  assert.equal(group({ text, entities: mention(text, '@pisper_bot') }).mentioned, true)
  assert.equal(
    group({ text: '/menu@Pisper_Bot', entities: [{ type: 'bot_command', offset: 0, length: 16 }] })
      .mentioned,
    true,
  )
  assert.equal(
    group({
      text: '值班',
      entities: [{ type: 'text_mention', offset: 0, length: 2, user: { id: 99 } }],
    }).mentioned,
    true,
  )
  assert.equal(group({ text: '2', reply_to_message: { from: { id: 99 } } }).mentioned, true)
  // @ 了别人、回复别人、私聊：都不标。
  const other = '@someone 值班'
  assert.equal(group({ text: other, entities: mention(other, '@someone') }).mentioned, undefined)
  assert.equal(group({ text: '2', reply_to_message: { from: { id: 5 } } }).mentioned, undefined)
  assert.equal(
    gateway.mapMessage({
      message_id: 2,
      chat: { id: 7, type: 'private' },
      from: { id: 7 },
      text: '@pisper_bot 2',
      entities: mention('@pisper_bot 2', '@pisper_bot'),
    }).mentioned,
    undefined,
  )
})

test('Feishu and QQ gateways mark group messages addressed to the bot the same way', async () => {
  const received = []
  let handlers = null
  const gateway = new FeishuGateway({
    onMessage: (message) => received.push(message),
    createChannelImpl: () => ({
      on: (value) => {
        handlers = value
      },
      connect: async () => {},
      disconnect: async () => {},
      getConnectionStatus: () => ({ state: 'connected' }),
    }),
  })
  await gateway.connect({ appId: 'a', appSecret: 's' })
  handlers.message({ chatId: 'oc_1', chatType: 'group', mentionedBot: true, content: '@_user_1 2' })
  handlers.message({ chatId: 'oc_1', chatType: 'p2p', mentionedBot: false, content: '2' })
  await gateway.disconnect()
  assert.equal(received[0].mentioned, true)
  assert.equal(received[0].peerId, 'oc_1')
  assert.equal(received[1].mentioned, undefined)

  const qq = new QQGateway({ onMessage: () => {} })
  const groupAt = qq.mapMessage('GROUP_AT_MESSAGE_CREATE', {
    id: 'm',
    group_openid: 'g',
    author: { id: 'u', member_openid: 'u' },
    content: ' 值班 2',
  })
  assert.equal(groupAt.mentioned, true)
  assert.equal(groupAt.content, '值班 2')
  const direct = qq.mapMessage('C2C_MESSAGE_CREATE', {
    id: 'm',
    author: { user_openid: 'u' },
    content: '2',
  })
  assert.equal(direct.mentioned, undefined)
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
