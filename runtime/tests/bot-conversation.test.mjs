// Bot 的对话（v2.1）：对话自己做快的事，长的、要批准的交给子 agent；它当经理用的手是 bot_* 工具。
// 工具输入的归一、状态快照、系统提示；运行时按工具派、补充、停子 agent（只碰自己的），巡看结果那一轮不开活，
// 提问卡片不等回答，定时只发给自己；每个 Bot 同时最多 3 个；不阻塞的接线（插队、不等批准、收尾在运行标记放下后）。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import {
  BOT_MAX_PARALLEL_AGENTS,
  botConversationPrompt,
  botStateSnapshot,
  normalizeBotAction,
  resolveBotAgent,
} from '../services/bots/bot-conversation.mjs'
import { botAnswerText, botScheduleInput } from '../services/bots/bot-schedule-input.mjs'
import { MultiAgentService, agentCompletionPrompt } from '../services/multi-agent-service.mjs'

const agent = (id, taskName, status, output = '') => ({ id, taskName, status, output, error: '' })

test('tool inputs are normalized strictly; anything malformed is refused', () => {
  assert.deepEqual(
    normalizeBotAction({
      type: 'spawn',
      title: '补退款测试',
      task: '在 src/checkout 里补退款与部分退款的测试，跑一遍后汇报',
      model: 'luna/gpt-5.6-luna',
      tier: 'strong',
    }),
    {
      type: 'spawn',
      title: '补退款测试',
      task: '在 src/checkout 里补退款与部分退款的测试，跑一遍后汇报',
      model: 'luna/gpt-5.6-luna',
      tier: 'strong',
    },
  )
  assert.deepEqual(
    normalizeBotAction({ type: 'spawn', title: 'x', task: 'y', tier: 'huge', model: 'bare' }),
    {
      type: 'spawn',
      title: 'x',
      task: 'y',
    },
  )
  assert.equal(normalizeBotAction({ type: 'spawn', title: '', task: '没有标题' }), null)
  assert.deepEqual(normalizeBotAction({ type: 'add', agent: 'a1b2', text: '边界值也测一下' }), {
    type: 'add',
    agent: 'a1b2',
    text: '边界值也测一下',
  })
  assert.equal(normalizeBotAction({ type: 'delete_everything' }), null)
  assert.deepEqual(
    normalizeBotAction({ type: 'remember', text: '别 push main', source: 'weird' }),
    {
      type: 'remember',
      text: '别 push main',
      source: 'told',
    },
  )
})

test('the state snapshot lists jobs, background commands, models and other bots', () => {
  const agents = [
    agent('a1b2c3d4-1', '补退款测试', 'running', '写完了退款用例，正在跑'),
    agent('e5f6a7b8-2', '依赖巡检', 'completed', '没有安全更新'),
  ]
  const snapshot = botStateSnapshot({
    agents,
    commands: [{ id: 'cmd_abc123', command: 'npm test', status: 'running' }],
    waitingApprovals: 1,
    models: ['luna/gpt-5.6-luna'],
    bots: [{ name: '测试', instructions: '补测试' }],
  })
  assert.match(snapshot, /^\[当前状态\]/)
  assert.match(snapshot, /现在在做 1 个，1 个操作在等用户批准/)
  assert.match(snapshot, /- \[a1b2c3d4\] 补退款测试 · running · 写完了退款用例，正在跑/)
  assert.match(snapshot, /最近结束：\n- \[e5f6a7b8\] 依赖巡检 · completed/)
  assert.match(snapshot, /后台命令：\n- \[cmd_abc123\] npm test/)
  assert.match(snapshot, /可以挑的模型.*luna\/gpt-5\.6-luna/)
  assert.match(snapshot, /可以找的别的 Bot：测试（补测试）/)
  assert.match(botStateSnapshot(), /手上没有活/)
  assert.equal(resolveBotAgent(agents, 'a1b2')?.id, 'a1b2c3d4-1')
  assert.equal(resolveBotAgent(agents, ''), null)
  assert.equal(resolveBotAgent(agents, 'zz'), null)
})

test('the prompt: quick work itself, long or approval-needing work to sub-agents, never confirm plans', () => {
  const prompt = botConversationPrompt({ name: '小橙', instructions: '负责 acme-web 前端' })
  assert.match(prompt, /你是 小橙/)
  assert.match(prompt, /快的事自己做/)
  assert.match(prompt, /bot_job_start/)
  assert.match(prompt, /bot_schedule 给自己定时；不要派一个干等的子 agent/)
  assert.match(prompt, /不要为了确认计划而停下来问/)
  assert.match(prompt, /先回应新消息/)
  assert.match(prompt, /也不要承诺系统没有的东西/)
  // 回复只留在对话里；要在聊天软件上告诉用户，用 bot_notify。
  assert.match(prompt, /bot_notify 发到他和机器人的私聊/)
  // 打开网页走右侧浏览器，不退回 shell 命令。
  assert.match(prompt, /browser_automation/)
  assert.match(prompt, /computer_use/)
  assert.match(prompt, /不要改用 shell 命令打开网页/)
  assert.match(prompt, /\[小橙 的工作说明\]\n负责 acme-web 前端/)
})

test('scheduling to yourself: once in N minutes, at a date, or repeating; always to this bot', () => {
  const bot = { id: 'bot_aaaaaaaaaaaa', name: '小橙' }
  const now = Date.parse('2026-11-04T17:20:30Z')
  assert.deepEqual(
    botScheduleInput({
      bot,
      input: { message: '看一下 Actions', inMinutes: 60 },
      now,
      timezone: 'Asia/Shanghai',
    }),
    {
      targetType: 'bot',
      botId: bot.id,
      botMessage: '看一下 Actions',
      name: '小橙：看一下 Actions',
      enabled: true,
      timezone: 'Asia/Shanghai',
      frequency: 'once',
      date: '2026-11-05',
      time: '02:21',
    },
  )
  assert.equal(
    botScheduleInput({
      bot,
      input: { message: '周报', repeat: 'weekly', time: '17:00', weekday: 5 },
      now,
    }).dayOfWeek,
    5,
  )
  assert.throws(() => botScheduleInput({ bot, input: { message: '' }, now }), /message/)
  assert.throws(() => botScheduleInput({ bot, input: { message: 'x' }, now }), /什么时候/)
  assert.equal(
    botAnswerText([{ question: '部署到哪？' }], {
      outcome: 'answered',
      answers: [{ selected: ['预览'] }],
    }),
    '部署到哪？ → 预览',
  )
})

// 只装 bot_* 工具运行时用得到的那几样。
function fakeRuntime({ agents = [], messages = [], unattended = false } = {}) {
  const calls = []
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  const bot = {
    id: 'bot_aaaaaaaaaaaa',
    name: '小橙',
    tools: null,
    threadSessionId: 'conv',
    instructions: '',
    enabled: true,
  }
  const value = {
    conversationOnly: true,
    unattendedRun: unattended,
    baseToolNames: ['read', 'edit', 'bash'],
    session: { model: { provider: 'luna', id: 'm' }, messages, systemPrompt: '' },
    multiAgentRuntime: {
      spawn: async (input) => {
        calls.push(['spawn', input])
        return { id: `0000000${calls.length}-new` }
      },
    },
  }
  Object.assign(runtime, {
    calls,
    value,
    sessions: new Map([['conv', value]]),
    bots: { list: () => [bot], get: () => bot },
    botForSession: (id) => (id === 'conv' ? bot : null),
    multiAgents: {
      list: () => agents,
      hasActive: () => true,
      followup: async (parent, id, text) => calls.push(['followup', parent, id, text]),
      interrupt: (parent, id) => calls.push(['interrupt', parent, id]),
    },
    providerPreferences: {
      availableChatModelRefs: async () => ['luna/m', 'nova/coder'],
      resolveSubagentModel: async ({ modelRef }) =>
        modelRef === 'nova/coder' ? { model: { provider: 'nova', id: 'coder' } } : { model: null },
    },
    botMemory: { list: async () => [], agentContext: async () => '' },
    schedules: {
      getState: () => ({
        tasks: [{ id: 'sch_1', targetType: 'bot', botId: bot.id, botMessage: 'x' }],
      }),
      create: async (input) => {
        calls.push(['schedule', input])
        return { id: 'sch_2', ...input }
      },
      remove: async (id) => calls.push(['unschedule', id]),
    },
    questions: {
      request: (input) => {
        calls.push(['question', input])
        return Promise.resolve({ outcome: 'answered', answers: [{ selected: ['预览'] }] })
      },
    },
    messageBot: async (botId, input) => calls.push(['messageBot', botId, input]),
  })
  return runtime
}

test('bot tools start, add to and stop only the bot’s own jobs, with its settings', async () => {
  const runtime = fakeRuntime({ agents: [agent('a1b2c3d4', '补退款测试', 'running')] })
  const tools = runtime.botConversationRuntime('conv')
  const started = await tools.startJob({
    title: '写更新说明',
    task: '整理这周的改动',
    model: 'nova/coder',
  })
  assert.match(started.text, /Started job 00000001/)
  await tools.startJob({ title: '跑回归', task: '跑一遍回归测试', tier: 'fast' })
  const [first, second] = runtime.calls
    .filter(([name]) => name === 'spawn')
    .map(([, input]) => input)
  // 工具按 Bot 的范围直接给（null = 全部），每个 Bot 同时最多 3 个，模型按活挑。
  assert.deepEqual(first.toolNames, ['read', 'edit', 'bash'])
  assert.equal(first.parentLimit, BOT_MAX_PARALLEL_AGENTS)
  assert.deepEqual(first.resolvedModel, { provider: 'nova', id: 'coder' })
  assert.equal(second.tier, 'fast')
  await tools.addToJob({ job: 'a1b2', text: '边界值也测一下' })
  await tools.stopJob({ job: 'a1b2' })
  assert.deepEqual(
    runtime.calls.find(([name]) => name === 'followup'),
    ['followup', 'conv', 'a1b2c3d4', '边界值也测一下'],
  )
  assert.deepEqual(
    runtime.calls.find(([name]) => name === 'interrupt'),
    ['interrupt', 'conv', 'a1b2c3d4'],
  )
  await assert.rejects(tools.stopJob({ job: 'not-mine' }), /对不上/)
  await assert.rejects(tools.startJob({ title: '', task: 'x' }), /参数/)
  // 不是 Bot 的对话：工具不可用。
  await assert.rejects(
    runtime.botConversationRuntime('other').startJob({ title: 'a', task: 'b' }),
    /不是 Bot 的对话/,
  )
})

test('a turn that reports a read-only check may not start work or message other bots', async () => {
  const done = agentCompletionPrompt({
    id: '11111111-2222-3333-4444-555555555555',
    taskName: 'proactive_check',
    status: 'completed',
    output: 'undici 有安全更新',
  })
  const runtime = fakeRuntime({
    messages: [{ role: 'user', content: [{ type: 'text', text: done }] }],
  })
  runtime.botProactiveAgents = new Set(['11111111-2222-3333-4444-555555555555'])
  const tools = runtime.botConversationRuntime('conv')
  await assert.rejects(tools.startJob({ title: '升级', task: '升级 undici' }), /read-only check/)
  await assert.rejects(tools.message({ bot: '测试', text: '帮我跑测试' }), /read-only check/)
  runtime.botMemory.remember = async (_bot, text) => ({ id: 'mem_aaaaaaaa', text })
  assert.equal((await tools.remember({ text: 'undici 有中危更新' })).text, 'Remembered.')
})

test('asking shows a card without waiting; the answer comes back as the next message', async () => {
  const runtime = fakeRuntime()
  const result = await runtime.botConversationRuntime('conv').ask({
    questions: [
      {
        question: '部署到哪？',
        options: [{ label: '预览' }, { label: '生产' }],
      },
    ],
  })
  assert.match(result.text, /End your turn here/)
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(runtime.calls[0][0], 'question')
  assert.deepEqual(runtime.calls[1], [
    'messageBot',
    'bot_aaaaaaaaaaaa',
    { text: '部署到哪？ → 预览', source: { kind: 'answer' } },
  ])
})

test('schedules go only to the bot itself, and never from an unattended turn', async () => {
  const runtime = fakeRuntime()
  const tools = runtime.botConversationRuntime('conv')
  const created = await tools.schedule({
    action: 'create',
    message: '看一下 Actions',
    inMinutes: 60,
  })
  assert.match(created.text, /Scheduled \(sch_2\)/)
  const [, input] = runtime.calls.find(([name]) => name === 'schedule')
  assert.equal(input.targetType, 'bot')
  assert.equal(input.botId, 'bot_aaaaaaaaaaaa')
  assert.equal(input.frequency, 'once')
  assert.match((await tools.schedule({ action: 'list' })).text, /sch_1/)
  await tools.schedule({ action: 'remove', id: 'sch_1' })
  assert.deepEqual(runtime.calls.at(-1), ['unschedule', 'sch_1'])
  await assert.rejects(tools.schedule({ action: 'remove', id: 'someone-else' }), /不是你的/)
  const unattended = fakeRuntime({ unattended: true }).botConversationRuntime('conv')
  await assert.rejects(
    unattended.schedule({ action: 'create', message: 'x', inMinutes: 5 }),
    /unattended/,
  )
})

test('after-turn work for one conversation runs one after another', async () => {
  const runtime = fakeRuntime()
  const order = []
  runtime.afterBotTurn = async (id) => {
    order.push(`start-${order.length}`)
    await new Promise((resolve) => setTimeout(resolve, 5))
    order.push(`end-${id}`)
  }
  await Promise.all([runtime.scheduleAfterBotTurn('conv'), runtime.scheduleAfterBotTurn('conv')])
  assert.deepEqual(order, ['start-0', 'end-conv', 'start-2', 'end-conv'])
})

test('each bot runs at most its own number of sub-agents at once; the rest wait', async () => {
  const started = []
  const service = new MultiAgentService({ path: '' })
  service.save = async () => {}
  service.startRun = async (record) => {
    started.push(record.taskName)
    return new Promise(() => {})
  }
  for (const name of ['one', 'two', 'three', 'four'])
    await service.spawn({
      parentSessionId: 'conv',
      cwd: '/repo',
      model: { provider: 'p', id: 'm' },
      taskName: name,
      message: `do ${name}`,
      parentLimit: 3,
    })
  await service.spawn({
    parentSessionId: 'other',
    cwd: '/repo',
    model: { provider: 'p', id: 'm' },
    taskName: 'other-one',
    message: 'do it',
  })
  assert.equal(started.length, 4)
  assert.ok(!started.includes('four'))
  assert.ok(started.includes('other-one'))
  assert.equal(service.list('conv').find((item) => item.taskName === 'four')?.status, 'queued')
})

test('the runtime wires a non-blocking bot conversation', async () => {
  const read = (path) => readFile(new URL(path, import.meta.url), 'utf8')
  const [runtime, activation, lifecycle, facade, methods] = await Promise.all([
    read('../runtime/agent-runtime.mjs'),
    read('../runtime/tool-activation.mjs'),
    read('../runtime/session-lifecycle.mjs'),
    read('../runtime/agent-runtime-facade.mjs'),
    read('../runtime/agent-session-methods.mjs'),
  ])
  // 对话有工具：Bot 的范围外加 bot_*；只挡有 bot_* 替代的多 Agent、ask_user、定时。
  assert.match(
    activation,
    /\.\.\.\(value\.conversationOnly \? BOT_CONVERSATION_TOOL_NAMES : \[\]\)/,
  )
  assert.match(
    activation,
    /export const BOT_CONVERSATION_BLOCKED_TOOLS = Object\.freeze\(\[\s*\.\.\.MULTI_AGENT_TOOL_NAMES,\s*ASK_USER_TOOL_NAME,/,
  )
  assert.match(
    runtime,
    /\.\.\.ISOLATED_CONTEXT_BLOCKED_TOOLS, \.\.\.BOT_CONVERSATION_BLOCKED_TOOLS/,
  )
  // 不阻塞：长命令转后台、子 agent 能做的不在对话里等批准、运行中发来的话插队。
  assert.match(runtime, /this\.botCommands\.wrap\(bashTool, \{ sessionId: runtimeSessionId \}\)/)
  assert.match(
    runtime,
    /noWait: \(toolName\) => this\.botApprovalRefusal\(runtimeSessionId, toolName\)/,
  )
  assert.match(
    methods,
    /if \(value\.conversationOnly && behavior === 'steer'\) behavior = 'interrupt'/,
  )
  assert.match(
    runtime,
    /if \(value\.conversationOnly\) contexts\.push\(await this\.botStateContext\(session\.sessionId\)\)/,
  )
  // 收尾等这一轮放下运行标记之后再排：这时交回来的完成通知不会 steer 进已结束的一轮而丢掉。
  assert.match(
    runtime,
    /if \(value\.conversationOnly && !error && !value\.abortedAt\) value\.botTurnDue = true/,
  )
  assert.match(
    facade,
    /value\.runActive = false[\s\S]*?if \(value\.botTurnDue\) \{\s*value\.botTurnDue = false\s*if \(!value\.abortedAt\) this\.scheduleAfterBotTurn\(id\)/,
  )
  assert.match(
    lifecycle,
    /if \(!value\?\.conversationOnly\) \{\s*this\.getMultiAgents\(\)\.abortParent\(id\)/,
  )
  // 改设定时运行时受保护（在跑的子 agent、后台命令、等唤醒的完成通知）就不释放。
  assert.match(
    facade,
    /if \(!this\.sessionRuntimeIsProtected\(sessionId, value\)\)\s*this\.disposeSessionRuntime/,
  )
  assert.match(lifecycle, /this\.getBotCommands\(\)\?\.hasActive\?\.\(id\)/)
})

test('bot_notify reaches the user’s own chats with the bot’s name, explains failures and is rate limited', async () => {
  const runtime = fakeRuntime()
  const outbox = []
  let results = [{ platform: 'qq', status: 'sent' }]
  runtime.channels = {
    notifyOwner: async (text, options) => {
      outbox.push([text, options])
      return results
    },
  }
  runtime.botNotifyTimes = new Map()
  const tools = runtime.botConversationRuntime('conv')
  const sent = await tools.notify({ text: ' 税务审核通过了 ', platform: 'qq' })
  assert.deepEqual(outbox[0], ['小橙：税务审核通过了', { platforms: ['qq'] }])
  assert.deepEqual(sent.sent, ['qq'])
  await tools.notify({ text: '全部渠道' })
  assert.deepEqual(outbox[1][1], { platforms: undefined })

  // 没发出去要说清楚为什么，模型才能如实告诉用户。
  results = [{ platform: 'qq', status: 'no_chat' }]
  await assert.rejects(
    tools.notify({ text: 'x' }),
    /never messaged the Pisper bot in a private chat/,
  )
  results = []
  await assert.rejects(tools.notify({ text: 'x' }), /no chat app is connected/)
  await assert.rejects(tools.notify({ text: '   ' }), /最多 2000/)
  await assert.rejects(
    runtime.botConversationRuntime('other').notify({ text: 'x' }),
    /不是 Bot 的对话/,
  )

  // 一小时最多 10 条，发失败的不算。
  results = [{ platform: 'qq', status: 'sent' }]
  for (let index = 2; index < 10; index += 1) await tools.notify({ text: `第 ${index} 条` })
  await assert.rejects(tools.notify({ text: '第 11 条' }), /already 10 notifications/)
})

test('bot_notify posts to groups in the bot’s rules only, picks the group it was asked from, and refuses others’ turns', async () => {
  const runtime = fakeRuntime()
  const bot = runtime.bots.get()
  bot.channels = [
    { platform: 'telegram', chatType: 'group', peerIds: null, senderIds: [], everyone: false },
    { platform: 'feishu', chatType: 'group', peerIds: ['oc_team'], senderIds: [], everyone: false },
    { platform: 'weixin', chatType: 'group', peerIds: ['wx_g'], senderIds: [], everyone: false },
    { platform: 'qq', chatType: 'p2p', peerIds: null, senderIds: [], everyone: false },
  ]
  const posted = []
  runtime.channels = {
    knownGroups: (platform) =>
      platform === 'telegram'
        ? [
            { platform, peerId: '-100', chatType: 'group', title: '发布群' },
            { platform, peerId: '-200', chatType: 'group', title: '' },
          ]
        : [],
    notifyGroup: async (...input) => posted.push(input),
  }
  runtime.botNotifyTimes = new Map()
  runtime.botChatOrigins = new Map()
  const tools = runtime.botConversationRuntime('conv')

  // 按群名或 ID 找；“所有群”的规则就是见过的那些群，写了群的规则就是写的那些。
  await tools.notify({ text: '发布好了', to: 'group', group: '发布群' })
  assert.deepEqual(posted.at(-1), ['telegram', '-100', '小橙：发布好了'])
  await tools.notify({ text: 'x', group: 'oc_team' })
  assert.deepEqual(posted.at(-1).slice(0, 2), ['feishu', 'oc_team'])
  // 微信没有群；不在规则里的群不发，并列出能发的群。
  await assert.rejects(tools.notify({ text: 'x', group: 'wx_g' }), /not one of your groups/)
  await assert.rejects(tools.notify({ text: 'x', group: '-999' }), /telegram:-100 \(发布群\)/)
  // 没说哪个：有几个群就要问清楚；这一轮来自某个群时就是那个群；限定平台后只剩一个也行。
  await assert.rejects(tools.notify({ text: 'x', to: 'group' }), /say which group/)
  runtime.botChatOrigins.set('conv', {
    kind: 'channel',
    platform: 'telegram',
    peerId: '-200',
    chatType: 'group',
  })
  await tools.notify({ text: 'x', to: 'group' })
  assert.deepEqual(posted.at(-1).slice(0, 2), ['telegram', '-200'])
  runtime.botChatOrigins.clear()
  await tools.notify({ text: 'x', to: 'group', platform: 'feishu' })
  assert.deepEqual(posted.at(-1).slice(0, 2), ['feishu', 'oc_team'])

  // 群里别人（或别的 Bot）叫起来的那一轮，不能让它去群里说。
  runtime.value.botTurnTrust = 'external'
  const before = posted.length
  await assert.rejects(tools.notify({ text: 'x', group: '发布群' }), /someone other than the user/)
  assert.equal(posted.length, before)
})

test('a schedule made from a group remembers which group it was', async () => {
  const runtime = fakeRuntime()
  runtime.botChatOrigins = new Map([
    ['conv', { kind: 'channel', platform: 'telegram', peerId: '-100', chatType: 'group' }],
  ])
  await runtime.botConversationRuntime('conv').schedule({
    action: 'create',
    message: '每天汇总一下进度',
    time: '18:00',
    repeat: 'daily',
  })
  const [, input] = runtime.calls.find(([name]) => name === 'schedule')
  assert.equal(input.botMessage, '每天汇总一下进度（在Telegram群里定的，群 ID -100）')
})
