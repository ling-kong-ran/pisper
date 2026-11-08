// 发回来源渠道（B5）：消息文字（回复、审批、提问）、聊天里的处理命令（批准 / 拒绝），以及渠道服务把
// 叫 Bot 的话交给它、在忙时直接说在忙。Bot 会话一侧（回复与审批回到哪个聊天）见 bot-sessions.test.mjs。
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { normalizeBotInput } from '../services/bots/bot-model.mjs'
import {
  botApprovalText,
  botChatAction,
  botQuestionText,
  botReplyText,
} from '../services/bots/bot-notifications.mjs'
import { ChannelService } from '../services/channels/channel-service.mjs'

test('chat messages carry the reply, the approval and the question', () => {
  assert.equal(botReplyText('审查员', { text: '  好了  ' }), '好了')
  assert.equal(botReplyText('审查员', { text: '' }), '审查员 做完了。')
  assert.equal(botReplyText('审查员', { error: 'boom' }), '审查员 没做成：boom')

  const standard = botApprovalText('审查员', {
    toolName: 'bash',
    args: { command: 'npm test' },
    severity: 'standard',
  })
  assert.match(standard, /需要批准：bash\nnpm test/)
  assert.match(standard, /\/审查员 批准 或 \/审查员 拒绝/)
  const critical = botApprovalText('审查员', {
    toolName: 'bash',
    args: { command: 'rm -rf /' },
    severity: 'critical',
  })
  assert.match(critical, /请在 Pisper 里看完再批准/)
  assert.doesNotMatch(critical, /\/审查员 批准/)

  assert.match(
    botQuestionText('助手', {
      questions: [{ question: '用哪种语言？', options: [{ label: '中文' }, { label: '英文' }] }],
    }),
    /助手 问：用哪种语言？\n· 中文\n· 英文\n请在 Pisper 里回答/,
  )

  // 只剩批准与拒绝：没有分支，也就没有应用 / 接受 / 放弃。
  assert.equal(botChatAction('批准'), 'approve')
  assert.equal(botChatAction(' No '), 'deny')
  assert.equal(botChatAction('应用'), null)
  assert.equal(botChatAction('放弃'), null)
  assert.equal(botChatAction('批准一下这个 PR 吧'), null)
})

class FakeGateway {
  constructor() {
    this.sent = []
    this.pushed = []
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
  async sendToPeer(peerId, input, scope) {
    this.pushed.push({ peerId, input, scope })
  }
  async sendAsset() {}
  async downloadResources() {
    return []
  }
}

test('a chat hands its words to the bot, says so when it is busy, and replies reuse the chat context', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-bot-chat-actions-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const weixin = new FakeGateway()
  const acts = []
  const messages = []
  let status = 'started'
  const bot = {
    id: 'b1',
    ...normalizeBotInput({
      name: '审查员',
      workspace: { cwd: '/repo' },
      channels: [{ platform: 'weixin', chatType: 'p2p', senderIds: ['u1'] }],
    }),
  }
  const idle = () => ({
    start: async () => ({}),
    get: () => null,
    cancel: () => false,
    dispose() {},
  })
  const service = new ChannelService({
    path: join(directory, 'channels.json'),
    cwd: directory,
    agent: {
      prompt: async () => ({ sessionId: 's', text: '', cwd: directory, assets: [] }),
      abort: async () => false,
      validateDirectory: async (value) => value,
    },
    bots: {
      enabled: () => true,
      list: () => [bot],
      activity: () => ({ status: 'running', waiting: 'approval', needsYou: true }),
      message: async (botId, input) => {
        messages.push([botId, input.text, input.source.kind])
        return { sessionId: 'thread-1', status }
      },
      act: async (botId, action) => {
        acts.push([botId, action])
        return '已批准，审查员 继续干活。'
      },
    },
    gatewayFactories: { feishu: () => new FakeGateway(), weixin: () => weixin },
    onboardingFactories: { feishu: idle, weixin: idle, qq: idle },
  })
  await service.init()
  await service.completeOnboarding('weixin', { botToken: 't', accountId: 'acc', ownerUserId: 'u1' })
  const say = (content, id) =>
    service.handleMessage('weixin', {
      messageId: id,
      peerId: 'u1',
      chatType: 'p2p',
      senderId: 'u1',
      content,
      contextToken: 'ctx-123',
      resources: [],
    })
  await say('/审查员 批准', 'm1')
  assert.deepEqual(acts, [['b1', 'approve']])
  assert.equal(weixin.sent.at(-1), '已批准，审查员 继续干活。')
  await say('/审查员 状态', 'm2')
  assert.equal(weixin.sent.at(-1), '审查员 在忙，正等人批准一个操作。')
  // 交给 Bot：不另发“收到了”，回复由 Runtime 在这一轮说完后发回来。
  const before = weixin.sent.length
  await say('/审查员 看看定时任务还有多久执行', 'm3')
  assert.deepEqual(messages, [['b1', '看看定时任务还有多久执行', 'channel']])
  assert.equal(weixin.sent.length, before)
  status = 'busy'
  await say('/审查员 在吗', 'm4')
  assert.match(weixin.sent.at(-1), /审查员 正在忙别的事/)
  assert.equal(await service.sendToChat('weixin', 'u1', '下一次在今天 18:00。'), true)
  assert.equal(weixin.pushed[0].scope.contextToken, 'ctx-123')
  assert.deepEqual(weixin.pushed[0].input, { text: '下一次在今天 18:00。' })
  assert.equal(await service.sendToChat('qq', 'x', 'hi'), false)
})

test('notifying the owner goes only to the owner’s own private chat, never a group or someone else', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-bot-notify-owner-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const weixin = new FakeGateway()
  const idle = () => ({
    start: async () => ({}),
    get: () => null,
    cancel: () => false,
    dispose() {},
  })
  const service = new ChannelService({
    path: join(directory, 'channels.json'),
    cwd: directory,
    agent: {
      prompt: async () => ({ sessionId: 's', text: '好的', cwd: directory, assets: [] }),
      abort: async () => false,
      validateDirectory: async (value) => value,
    },
    gatewayFactories: { feishu: () => new FakeGateway(), weixin: () => weixin },
    onboardingFactories: { feishu: idle, weixin: idle, qq: idle },
  })
  await service.init()
  // 还没连接任何渠道、或创建者从没私聊过：发不出去，结果写明原因。
  assert.deepEqual(await service.notifyOwner('x'), [])
  await service.completeOnboarding('weixin', { botToken: 't', accountId: 'acc', ownerUserId: 'u1' })
  assert.deepEqual(await service.notifyOwner('x'), [{ platform: 'weixin', status: 'no_chat' }])

  await service.handleMessage('weixin', {
    messageId: 'm1',
    peerId: 'u1',
    chatType: 'p2p',
    senderId: 'u1',
    content: '你好',
    contextToken: 'ctx-owner',
    resources: [],
  })
  // 之后别人的私聊、创建者所在的群更晚说过话，也不能发过去。
  const later = new Date(Date.now() + 60_000).toISOString()
  service.state.scopes['weixin:u2'] = {
    platform: 'weixin',
    peerId: 'u2',
    chatType: 'p2p',
    senderId: 'u2',
    updatedAt: later,
  }
  service.state.scopes['weixin:g1'] = {
    platform: 'weixin',
    peerId: 'g1',
    chatType: 'group',
    senderId: 'u1',
    updatedAt: later,
  }
  assert.deepEqual(await service.notifyOwner('审核通过了'), [
    { platform: 'weixin', status: 'sent' },
  ])
  assert.equal(weixin.pushed.at(-1).peerId, 'u1')
  assert.deepEqual(weixin.pushed.at(-1).input, { text: '审核通过了' })
  assert.equal(weixin.pushed.at(-1).scope.contextToken, 'ctx-owner')
  // 公开出去的会话列表不带发送者身份。
  assert.equal(
    service.getState().scopes.some((scope) => 'senderId' in scope),
    false,
  )
  // 旧记录没写发送者：渠道只给创建者用时算他的；对所有人开放时不认。
  delete service.state.scopes['weixin:u1'].senderId
  assert.equal(service.ownerChat('weixin')?.peerId, 'u1')
  service.state.connections.weixin.accessMode = 'all'
  assert.equal(service.ownerChat('weixin'), null)
  // 只发指定平台；没连接的平台不出现在结果里。
  assert.deepEqual(await service.notifyOwner('x', { platforms: ['qq'] }), [])
})

test('group pushes use the group’s own chat type, remember Telegram group names and never go to WeChat', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-bot-notify-group-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const weixin = new FakeGateway()
  const idle = () => ({
    start: async () => ({}),
    get: () => null,
    cancel: () => false,
    dispose() {},
  })
  const service = new ChannelService({
    path: join(directory, 'channels.json'),
    cwd: directory,
    agent: {
      prompt: async () => ({ sessionId: 's', text: '好的', cwd: directory, assets: [] }),
      abort: async () => false,
      validateDirectory: async (value) => value,
    },
    gatewayFactories: { feishu: () => new FakeGateway(), weixin: () => weixin },
    onboardingFactories: { feishu: idle, weixin: idle, qq: idle },
  })
  await service.init()
  await service.completeOnboarding('weixin', { botToken: 't', accountId: 'acc', ownerUserId: 'u1' })
  service.state.scopes['weixin:g1'] = {
    platform: 'weixin',
    peerId: 'g1',
    chatType: 'group',
    chatTitle: '旧名',
    updatedAt: '2026-01-01T00:00:00.000Z',
  }
  service.botPeers.set('weixin:g1', {
    platform: 'weixin',
    peerId: 'g1',
    chatType: 'group',
    chatTitle: '发布群',
    updatedAt: '2026-02-01T00:00:00.000Z',
  })
  // 同一个群只列一次，名字取最近的；私聊不算群。
  assert.deepEqual(
    service.knownGroups('weixin').map(({ peerId, title }) => [peerId, title]),
    [['g1', '发布群']],
  )
  await assert.rejects(service.notifyGroup('weixin', 'g1', 'x'), /微信只支持私聊/)
  await assert.rejects(service.notifyGroup('telegram', '-100', 'x'), /没有连接/)
  assert.equal(weixin.pushed.length, 0)
})
