// 渠道与定时任务里的 Bot（B3）：点名规则、名单校验、渠道命令、定时任务的 bot 目标。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { normalizeBotInput } from '../services/bots/bot-model.mjs'
import { botRefusal, parseBotMention, routeChannelMessage } from '../services/bots/bot-routing.mjs'
import { ChannelService } from '../services/channels/channel-service.mjs'
import { ScheduleService } from '../services/schedule-service.mjs'

function bot(id, input) {
  return { id, ...normalizeBotInput({ workspace: { cwd: '/repo' }, ...input }) }
}

const reviewer = bot('b1', {
  name: '审查员',
  channels: [
    { platform: 'feishu', chatType: 'group', peerIds: ['team'], senderIds: ['alice'] },
    { platform: 'feishu', chatType: 'p2p' },
  ],
})
const review = bot('b2', {
  name: '审查',
  channels: [{ platform: 'feishu', chatType: 'p2p' }],
})
const paused = bot('b3', {
  name: 'Fixer',
  enabled: false,
  channels: [{ platform: 'telegram', chatType: 'p2p' }],
})
const helper = bot('b4', {
  name: '助手',
  channels: [{ platform: 'telegram', chatType: 'p2p', senderIds: ['u1'] }],
  defaultIn: ['telegram:dm1'],
})
const bots = [reviewer, review, paused, helper]
const group = { platform: 'feishu', peerId: 'team', chatType: 'group', senderId: 'alice' }

test('bots are named by /name, @name or right after @Pisper, longest name first', () => {
  assert.deepEqual(pick(parseBotMention('/审查员 看下 PR', bots)), ['审查员', '看下 PR'])
  assert.deepEqual(pick(parseBotMention('@审查员：看下 PR', bots)), ['审查员', '看下 PR'])
  assert.deepEqual(pick(parseBotMention('@Pisper 审查员 看下 PR', bots)), ['审查员', '看下 PR'])
  assert.deepEqual(pick(parseBotMention('@_user_1 审查 这个', bots)), ['审查', '这个'])
  assert.deepEqual(pick(parseBotMention('/fixer 修一下', bots)), ['Fixer', '修一下'])
  // 名字后面必须是分隔符：“审查员们”不是点名。
  assert.equal(parseBotMention('/审查员们 看下', bots).bot, null)
  // 没有 @ 的群消息里提到名字不算点名。
  assert.equal(parseBotMention('审查员 看下', bots).bot, null)
  assert.equal(parseBotMention('/new', bots).bot, null)
})

test('only enabled bots, in their chats, for allowed people', () => {
  assert.deepEqual(
    pickRoute(routeChannelMessage({ ...group, content: '@Pisper 审查员 看下' }, bots)),
    ['bot', '审查员', '看下'],
  )
  assert.equal(
    routeChannelMessage({ ...group, senderId: 'mallory', content: '/审查员 看下' }, bots).reason,
    'sender_not_allowed',
  )
  assert.equal(
    routeChannelMessage({ ...group, peerId: 'other', content: '/审查员 看下' }, bots).reason,
    'not_in_chat',
  )
  assert.equal(
    routeChannelMessage(
      { platform: 'telegram', peerId: 'x', chatType: 'p2p', senderId: 'u1', content: '/Fixer go' },
      bots,
    ).reason,
    'paused',
  )
  // 私聊没写名单：只有渠道创建者能叫，渠道“所有人可用”不放宽。
  const dm = { platform: 'feishu', peerId: 'dm', chatType: 'p2p', content: '审查，看下' }
  assert.equal(
    routeChannelMessage({ ...dm, senderId: 'stranger' }, bots, { ownerId: 'me' }).reason,
    'sender_not_allowed',
  )
  assert.equal(routeChannelMessage({ ...dm, senderId: 'me' }, bots, { ownerId: 'me' }).kind, 'bot')
  assert.equal(botRefusal(review, { ...dm, senderId: 'me' }, { ownerId: '' }), 'sender_not_allowed')
})

test('a default bot takes unnamed direct messages; other messages stay with Pisper', () => {
  const dm = { platform: 'telegram', peerId: 'dm1', chatType: 'p2p', senderId: 'u1' }
  assert.deepEqual(pickRoute(routeChannelMessage({ ...dm, content: '跑一下测试' }, bots)), [
    'bot',
    '助手',
    '跑一下测试',
  ])
  assert.equal(routeChannelMessage({ ...dm, peerId: 'dm2', content: '你好' }, bots).kind, 'none')
  assert.equal(routeChannelMessage({ ...group, content: '大家好' }, bots).kind, 'none')
  // 斜杠命令不会落到默认 Bot。
  assert.equal(routeChannelMessage({ ...dm, content: '/whatever' }, bots).kind, 'none')
})

function pick(result) {
  return [result.bot?.name, result.text]
}

function pickRoute(result) {
  return [result.kind, result.bot?.name, result.text]
}

class FakeGateway {
  constructor() {
    this.sent = []
  }
  getStatus() {
    return { state: 'connected', lastError: '', bot: null }
  }
  async connect() {
    return this.getStatus()
  }
  async disconnect() {}
  async send(message, input) {
    this.sent.push(input.text || input.markdown)
  }
  async sendToPeer() {}
  async sendAsset() {}
  async downloadResources() {
    return []
  }
}

async function channelFixture(t, { enabled = true, owner = 'me' } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-bot-channels-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const gateway = new FakeGateway()
  const prompts = []
  const messages = []
  const service = new ChannelService({
    path: join(directory, 'channels.json'),
    cwd: directory,
    agent: {
      prompt: async (input) => {
        prompts.push(input.message)
        return { sessionId: 's', text: 'Pisper 回复', cwd: directory, assets: [] }
      },
      abort: async () => false,
      validateDirectory: async (value) => value,
    },
    bots: {
      enabled: () => enabled,
      list: () => bots,
      activity: () => ({ status: 'running', waiting: null, needsYou: false }),
      message: async (botId, input) => {
        messages.push({ botId, ...input })
        return { sessionId: 'thread-1', status: 'started' }
      },
    },
    gatewayFactories: { feishu: () => gateway, weixin: () => new FakeGateway() },
    onboardingFactories: {
      feishu: () => ({
        start: async () => ({}),
        get: () => null,
        cancel: () => false,
        dispose() {},
      }),
      weixin: () => ({
        start: async () => ({}),
        get: () => null,
        cancel: () => false,
        dispose() {},
      }),
      qq: () => ({ start: async () => ({}), get: () => null, cancel: () => false, dispose() {} }),
    },
  })
  await service.init()
  await service.completeOnboarding('feishu', {
    appId: 'cli_app',
    appSecret: 's',
    ownerOpenId: owner,
  })
  await service.update('feishu', { accessMode: 'all' })
  const send = (content, extra = {}) =>
    service.handleMessage('feishu', {
      messageId: 'm',
      peerId: 'team',
      chatType: 'group',
      senderId: 'alice',
      senderName: 'Alice',
      content,
      resources: [],
      ...extra,
    })
  return { gateway, prompts, messages, send }
}

test('channel messages that name a bot go into its session; the rest stay direct chat', async (t) => {
  const { gateway, prompts, messages, send } = await channelFixture(t)
  await send('@Pisper 审查员 看下这个 PR')
  assert.equal(messages[0].botId, 'b1')
  assert.equal(messages[0].text, '看下这个 PR')
  assert.deepEqual(messages[0].source, {
    kind: 'channel',
    platform: 'feishu',
    peerId: 'team',
    chatType: 'group',
    senderId: 'alice',
    senderName: 'Alice',
    messageId: 'm',
    // alice 不是渠道绑定的用户本人：她的话只算外部说法。
    fromOwner: false,
  })
  // 不另发“收到了”：回复由 Runtime 在这一轮说完后发回这个聊天。
  assert.equal(gateway.sent.length, 0)

  await send('/审查员 看下', { senderId: 'mallory' })
  assert.match(gateway.sent.at(-1), /不在 审查员 的名单里/)
  assert.equal(messages.length, 1)

  await send('/审查员 状态')
  assert.equal(gateway.sent.at(-1), '审查员 正在忙。')

  await send('/bots')
  assert.match(gateway.sent.at(-1), /审查员/)
  await send('/whoami')
  assert.match(gateway.sent.at(-1), /这个聊天：team\n你的 ID：alice/)

  await send('大家好')
  assert.deepEqual(prompts, ['大家好'])
})

test('a message from the channel’s owner is marked as theirs', async (t) => {
  const { messages, send } = await channelFixture(t, { owner: 'alice' })
  await send('@Pisper 审查员 看下这个 PR')
  assert.equal(messages[0].source.fromOwner, true)
})

test('with Bots off, naming a bot is ordinary chat', async (t) => {
  const { prompts, messages, send } = await channelFixture(t, { enabled: false })
  await send('/审查员 看下')
  assert.equal(messages.length, 0)
  assert.deepEqual(prompts, ['/审查员 看下'])
})

test('schedules talk to a bot in its session without ever running a full-access prompt', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-bot-schedules-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const dispatched = []
  let enabled = true
  const service = new ScheduleService({
    path: join(directory, 'schedules.json'),
    cwd: directory,
    agent: {
      prompt: async () => {
        throw new Error('bot schedules must not run as prompts')
      },
      validateDirectory: async (value) => value,
    },
    notifications: { notify: async () => {} },
    bots: {
      enabled: () => enabled,
      get: (id) => (id === 'b1' ? reviewer : null),
      message: async (botId, input) => {
        dispatched.push({ botId, ...input })
        return { sessionId: 'thread-1', status: 'started' }
      },
    },
    tickMs: 60_000,
  })
  await service.init()
  t.after(() => service.dispose())
  await assert.rejects(
    service.create({ name: 'x', targetType: 'bot', botId: 'nope', botMessage: '查依赖' }),
    /Bot 不存在/,
  )
  await assert.rejects(
    service.create({ name: 'x', targetType: 'bot', botId: 'b1', botMessage: ' ' }),
    /要让 Bot 做什么/,
  )
  const task = await service.create({
    name: '每天查依赖',
    targetType: 'bot',
    botId: 'b1',
    botMessage: '查依赖有没有安全更新',
    prompt: '这段不该被保存',
    frequency: 'daily',
    time: '09:00',
  })
  assert.equal(task.prompt, '')
  assert.equal(task.botMessage, '查依赖有没有安全更新')
  const stored = JSON.parse(await readFile(join(directory, 'schedules.json'), 'utf8'))
  assert.equal(stored.tasks[0].prompt, '')

  await service.runNow(task.id)
  await service.idle?.()
  await waitFor(() => dispatched.length === 1)
  assert.deepEqual(dispatched[0], {
    botId: 'b1',
    text: '查依赖有没有安全更新',
    source: { kind: 'schedule', scheduleTaskId: task.id },
  })

  enabled = false
  await assert.rejects(
    service.create({ name: 'y', targetType: 'bot', botId: 'b1', botMessage: '查' }),
    /实验功能/,
  )
  enabled = true
  assert.equal(await service.deleteForBot('b1'), 1)
  assert.equal(service.getState().tasks.length, 0)
})

async function waitFor(predicate, timeoutMs = 5_000) {
  const started = Date.now()
  while (!predicate()) {
    if (Date.now() - started > timeoutMs) throw new Error('timed out')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}
