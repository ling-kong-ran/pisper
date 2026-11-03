// 回推到来源渠道（B5）：消息文字、任务事件、聊天里的处理命令（批准 / 拒绝 / 应用 / 接受 / 放弃）。
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { normalizeBotInput } from '../services/bots/bot-model.mjs'
import {
  botApprovalText,
  botChatAction,
  botQuestionText,
  botTaskEventText,
} from '../services/bots/bot-notifications.mjs'
import { BotTaskService } from '../services/bots/bot-task-service.mjs'
import { ChannelService } from '../services/channels/channel-service.mjs'

const facade = AgentRuntimeFacade.prototype

test('chat messages say what happened and what you can reply', () => {
  const review = botTaskEventText(
    '审查员',
    {
      title: '修按钮',
      result: { summary: '按钮不换行了', changedFiles: 2 },
      workspace: { branch: 'pisper/x/1' },
    },
    'review',
  )
  assert.match(review, /审查员 改好了「修按钮」：2 个文件有改动，在分支 pisper\/x\/1 上/)
  assert.match(review, /\/审查员 应用（接受并应用到当前分支）/)
  assert.match(
    botTaskEventText('审查员', { title: 't', result: { summary: '好了' } }, 'done'),
    /做完了「t」。\n好了/,
  )
  assert.match(botTaskEventText('审查员', { error: { message: 'boom' } }, 'failed'), /没做成：boom/)
  assert.equal(botTaskEventText('审查员', {}, 'queued'), '')

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

  assert.equal(botChatAction('批准'), 'approve')
  assert.equal(botChatAction(' Apply '), 'apply')
  assert.equal(botChatAction('放弃'), 'discard')
  assert.equal(botChatAction('批准一下这个 PR 吧'), null)
})

test('task results fire one event each; stopping from the app does not', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-bot-events-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const events = []
  let fail = false
  const bot = {
    id: 'b1',
    ...normalizeBotInput({
      name: '助手',
      delivery: 'reply-only',
      workspace: { cwd: directory, isolation: 'shared' },
    }),
  }
  const service = new BotTaskService({
    path: join(directory, 'tasks.json'),
    getBot: () => bot,
    worktrees: null,
    runAgent: async () => {
      if (fail) throw new Error('模型不可用')
      return { text: '好了', sessionId: 's' }
    },
    stopAgent: async () => {},
    onTaskEvent: (task, kind) => events.push([kind, task.id]),
  })
  t.after(() => service.dispose())
  await service.init()
  const done = await service.dispatch('b1', { text: '一' })
  await waitFor(() => events.length === 1)
  assert.deepEqual(events[0], ['done', done.id])
  fail = true
  const failed = await service.dispatch('b1', { text: '二' })
  await waitFor(() => events.length === 2)
  assert.deepEqual(events[1], ['failed', failed.id])
})

test('chat commands approve, refuse critical steps, and accept or discard changes', async () => {
  const calls = []
  let approval = { id: 'a1', severity: 'standard' }
  const runtime = {
    bots: { get: () => ({ id: 'b1', name: '审查员' }) },
    botTasks: {
      latest: (_botId, statuses) =>
        statuses.includes('running')
          ? { id: 't1', sessionId: 's1' }
          : { id: 't2', status: 'review' },
      accept: async (id, options) => {
        calls.push(['accept', id, options])
        return { result: { applied: options.apply, branch: 'pisper/x/1' } }
      },
      discard: async (id) => calls.push(['discard', id]),
    },
    botTaskWaiting: () => ({ approval, question: null }),
    resolveToolApproval: (sessionId, id, approved) => {
      calls.push(['resolve', sessionId, id, approved])
      return { found: true }
    },
  }
  const act = (action) => facade.botChannelAction.call(runtime, 'b1', action)
  assert.match(await act('approve'), /已批准/)
  assert.deepEqual(calls.pop(), ['resolve', 's1', 'a1', true])
  approval = { id: 'a2', severity: 'critical' }
  assert.match(await act('approve'), /高风险/)
  assert.equal(calls.length, 0)
  // 高风险操作可以在聊天里拒绝。
  assert.match(await act('deny'), /已拒绝/)
  assert.deepEqual(calls.pop(), ['resolve', 's1', 'a2', false])
  assert.match(await act('apply'), /已接受并应用到当前分支（分支 pisper\/x\/1 保留）/)
  assert.deepEqual(calls.pop(), ['accept', 't2', { apply: true }])
  assert.match(await act('accept'), /改动在分支 pisper\/x\/1 上/)
  assert.match(await act('discard'), /已放弃/)
  approval = null
  assert.match(await act('approve'), /没有等批准的操作/)
})

test('only tasks that came from a chat report back, to that chat', () => {
  const sent = []
  const runtime = {
    capabilities: { features: { bots: true } },
    isBetaFeatureEnabled: () => true,
    channels: {
      sendToChat: async (platform, peerId, text) => {
        sent.push({ platform, peerId, text })
        return true
      },
    },
    bots: { get: () => ({ name: '审查员' }) },
    botTasks: {
      runningForSession: (sessionId) =>
        sessionId === 'from-chat'
          ? { botId: 'b1', source: { kind: 'channel', platform: 'feishu', peerId: 'team' } }
          : sessionId === 'from-app'
            ? { botId: 'b1', source: { kind: 'chat' } }
            : null,
    },
    notifyBotChat: facade.notifyBotChat,
  }
  facade.botSessionEvent.call(runtime, 'from-chat', 'permission_request', {
    toolName: 'bash',
    args: { command: 'npm test' },
    severity: 'standard',
  })
  facade.botSessionEvent.call(runtime, 'from-app', 'permission_request', { toolName: 'bash' })
  facade.botSessionEvent.call(runtime, 'from-chat', 'permission_resolved', {})
  facade.botTaskEvent.call(
    runtime,
    {
      botId: 'b1',
      title: '修',
      result: { summary: '好了' },
      source: { kind: 'channel', platform: 'telegram', peerId: 'dm' },
    },
    'done',
  )
  assert.deepEqual(
    sent.map((item) => [item.platform, item.peerId]),
    [
      ['feishu', 'team'],
      ['telegram', 'dm'],
    ],
  )
  assert.match(sent[0].text, /需要批准：bash/)
  assert.match(sent[1].text, /审查员 做完了「修」/)
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

test('replies to a bot in chat run the command, and push-backs reuse the chat context', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-bot-chat-actions-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const weixin = new FakeGateway()
  const acts = []
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
      activity: () => null,
      message: async () => ({ turns: [{}] }),
      act: async (botId, action) => {
        acts.push([botId, action])
        return '已接受并应用到当前分支。'
      },
    },
    gatewayFactories: { feishu: () => new FakeGateway(), weixin: () => weixin },
    onboardingFactories: { feishu: idle, weixin: idle, qq: idle },
  })
  await service.init()
  await service.completeOnboarding('weixin', { botToken: 't', accountId: 'acc', ownerUserId: 'u1' })
  await service.handleMessage('weixin', {
    messageId: 'm1',
    peerId: 'u1',
    chatType: 'p2p',
    senderId: 'u1',
    content: '/审查员 应用',
    contextToken: 'ctx-123',
    resources: [],
  })
  assert.deepEqual(acts, [['b1', 'apply']])
  assert.equal(weixin.sent.at(-1), '已接受并应用到当前分支。')
  assert.equal(await service.sendToChat('weixin', 'u1', '审查员 做完了'), true)
  assert.equal(weixin.pushed[0].scope.contextToken, 'ctx-123')
  assert.deepEqual(weixin.pushed[0].input, { text: '审查员 做完了' })
  assert.equal(await service.sendToChat('qq', 'x', 'hi'), false)
})

async function waitFor(predicate, timeoutMs = 5_000) {
  const started = Date.now()
  while (!predicate()) {
    if (Date.now() - started > timeoutMs) throw new Error('timed out')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}
