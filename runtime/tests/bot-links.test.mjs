// Bot 之间的来往（C5）：一个 Bot 给另一个 Bot 发话，话进对方的对话（标着来自谁）；对方的回复、
// 替它做的活的结果再交回来；同一条链最多 6 跳、每对 Bot 每分钟有上限、客气话不发；权限不传递。
import assert from 'node:assert/strict'
import test from 'node:test'

import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { botTurnTrigger } from '../runtime/bot-decisions.mjs'
import { ATTACHMENT_MARKER, serializeTranscriptMessages } from '../runtime/stream-projection.mjs'
import { normalizeBotAction } from '../services/bots/bot-conversation.mjs'
import {
  BotLinkService,
  MAX_BOT_LINK_HOPS,
  MAX_BOT_LINK_MESSAGES_PER_MINUTE,
  botMessageContext,
  isCourtesyMessage,
  parseBotMessageTag,
} from '../services/bots/bot-links.mjs'
import { agentCompletionPrompt } from '../services/multi-agent-service.mjs'

const A = { id: 'bot_aaaaaaaaaaaa', name: '小橙', enabled: true, tools: null, instructions: '前端' }
const B = {
  id: 'bot_bbbbbbbbbbbb',
  name: '测试',
  enabled: true,
  tools: ['read', 'bash'],
  instructions: '补测试',
}

test('the source tag round-trips and courtesy messages are recognised', () => {
  const context = botMessageContext({
    fromName: '小橙',
    from: A.id,
    chain: 'ch_abcdefghij',
    hop: 2,
    reply: true,
  })
  assert.deepEqual(parseBotMessageTag(context), {
    from: A.id,
    chain: 'ch_abcdefghij',
    hop: 2,
    reply: true,
  })
  assert.match(context, /from the bot "小橙", not from the user/)
  assert.equal(parseBotMessageTag('普通的话'), null)
  for (const text of ['收到', '好的！', '谢谢', 'OK', 'thanks', '👍', '']) {
    assert.equal(isCourtesyMessage(text), true, text)
  }
  for (const text of ['好的，我先跑一遍回归', '收到，结果是 3 个失败']) {
    assert.equal(isCourtesyMessage(text), false, text)
  }
  assert.deepEqual(
    [
      { type: 'message', bot: '测试', text: '帮我跑一遍回归' },
      { type: 'message', bot: '测试', text: '不用回', expectsReply: false },
      { type: 'message', bot: '', text: 'x' },
    ].map(normalizeBotAction),
    [
      { type: 'message', bot: '测试', text: '帮我跑一遍回归', expectsReply: true },
      { type: 'message', bot: '测试', text: '不用回', expectsReply: false },
      null,
    ],
  )
})

test('links stop at the hop limit, the per-pair rate and never go to yourself', () => {
  let now = 0
  const links = new BotLinkService({ now: () => now })
  assert.deepEqual(links.admit({ from: A.id, to: A.id, chain: 'ch_x', hop: 1 }), {
    ok: false,
    reason: 'self',
  })
  assert.equal(
    links.admit({ from: A.id, to: B.id, chain: 'ch_x', hop: MAX_BOT_LINK_HOPS }).ok,
    true,
  )
  assert.equal(
    links.admit({ from: A.id, to: B.id, chain: 'ch_x', hop: MAX_BOT_LINK_HOPS + 1 }).reason,
    'hops',
  )
  for (let index = 1; index < MAX_BOT_LINK_MESSAGES_PER_MINUTE; index += 1)
    assert.equal(links.admit({ from: B.id, to: A.id, chain: 'ch_y', hop: 1 }).ok, true)
  assert.equal(links.admit({ from: A.id, to: B.id, chain: 'ch_z', hop: 1 }).reason, 'rate')
  now += 61_000
  assert.equal(links.admit({ from: A.id, to: B.id, chain: 'ch_z', hop: 1 }).ok, true)
  links.open({ chain: 'ch_z', from: A.id, to: B.id, text: '帮我跑一遍回归' })
  assert.deepEqual(
    links.listForBot(A.id).map((link) => [link.direction, link.botId, link.status]),
    [['out', B.id, 'waiting']],
  )
  // 别人找它的只在停下来时出现在它的时间线上。
  assert.deepEqual(links.listForBot(B.id), [])
  links.stop('ch_z', 'hops')
  assert.deepEqual(
    links.listForBot(B.id).map((link) => [link.direction, link.botId, link.status]),
    [['in', A.id, 'stopped']],
  )
})

function userMessage(text) {
  return { role: 'user', content: [{ type: 'text', text }], timestamp: 1 }
}

function assistantMessage(text) {
  return { role: 'assistant', content: [{ type: 'text', text }], timestamp: 2 }
}

test('the turn trigger reads the tag only from injected context, plus finished jobs', () => {
  const tagged = `帮我跑一遍回归${ATTACHMENT_MARKER}${botMessageContext({ fromName: '小橙', from: A.id, chain: 'ch_abcdefghij', hop: 1, reply: true })}`
  assert.deepEqual(botTurnTrigger([userMessage(tagged), assistantMessage('好，我派人去跑。')]), {
    message: { from: A.id, chain: 'ch_abcdefghij', hop: 1, reply: true },
    brief: false,
    completedAgents: [],
    reply: '好，我派人去跑。',
  })
  // 用户自己在正文里写标记不算。
  const typed = `[pisper:bot-message from=${A.id} chain=ch_abcdefghij hop=1 reply=1]`
  assert.equal(botTurnTrigger([userMessage(typed), assistantMessage('嗯')]).message, null)
  const done = agentCompletionPrompt({
    id: '11111111-2222-3333-4444-555555555555',
    taskName: 'regression',
    status: 'completed',
    output: '3 个失败',
  })
  assert.deepEqual(
    botTurnTrigger([userMessage(done), assistantMessage('回归跑完了：3 个失败。')]).completedAgents,
    ['11111111-2222-3333-4444-555555555555'],
  )
  // 转录里这条话显示发话 Bot；用户正文里写的标记不算。
  const [fromBot, plain] = serializeTranscriptMessages([userMessage(tagged), userMessage(typed)])
  assert.equal(fromBot.fromBot, A.id)
  assert.equal(fromBot.text, '帮我跑一遍回归')
  assert.equal(plain.fromBot, undefined)
})

// 两个 Bot：A 的对话、B 的对话；messageBot 只记下投进去的话，spawn 只记下派活的参数。
function twoBots() {
  const delivered = []
  const spawned = []
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  const bots = [A, B]
  Object.assign(runtime, {
    delivered,
    spawned,
    bots: { list: () => bots, get: (id) => bots.find((bot) => bot.id === id) || null },
    botLinks: new BotLinkService(),
    botMemory: { list: async () => [], agentContext: async () => '' },
    multiAgents: { list: () => [] },
    providerPreferences: { resolveSubagentModel: async () => ({ model: null }) },
    messageBot: async (botId, input) => {
      delivered.push({ botId, ...input })
      return { status: 'started' }
    },
  })
  return runtime
}

const sessionValue = (spawned) => ({
  baseToolNames: ['read', 'edit', 'bash'],
  multiAgentRuntime: {
    spawn: async (input) => {
      spawned.push(input)
      return { id: '11111111-2222-3333-4444-555555555555' }
    },
  },
})

test('A asks B for work; B’s reply and the result of the work it does come back to A', async () => {
  const runtime = twoBots()
  // A 的对话决定找 B。
  const sent = await runtime.executeBotDecision(
    'conv-a',
    {},
    A,
    { type: 'message', bot: '测试', text: '帮我跑一遍回归，结果发我', expectsReply: true },
    null,
  )
  assert.equal(runtime.delivered[0].botId, B.id)
  assert.equal(runtime.delivered[0].source.kind, 'bot')
  const tag = parseBotMessageTag(runtime.delivered[0].text)
  assert.deepEqual(tag, { from: A.id, chain: sent.chain, hop: 1, reply: true })
  assert.match(runtime.delivered[0].text, /^帮我跑一遍回归，结果发我\n\n---\n/)
  assert.deepEqual(
    runtime.listSessionLinks.call(
      Object.assign(Object.create(runtime), { botForSession: () => A }),
    ),
    runtime.botLinks.listForBot(A.id),
  )
  assert.equal(runtime.botLinks.get(sent.chain).status, 'waiting')

  // B 这一轮：回一句，派一个子 agent（用 B 自己的工具范围，不是 A 的）。
  const turn = { message: tag, completedAgents: [], reply: '好，我派人去跑回归。' }
  await runtime.forwardBotReplies(B, {}, turn)
  assert.equal(runtime.delivered[1].botId, A.id)
  assert.deepEqual(parseBotMessageTag(runtime.delivered[1].text), {
    from: B.id,
    chain: sent.chain,
    hop: 2,
    reply: false,
  })
  await runtime.executeBotDecision(
    'conv-b',
    sessionValue(runtime.spawned),
    B,
    { type: 'spawn', title: '跑回归', task: '跑一遍回归测试' },
    turn,
  )
  assert.deepEqual(runtime.spawned[0].toolNames, ['read', 'bash'])
  assert.equal(runtime.botLinks.get(sent.chain).status, 'waiting')

  // 活做完，B 汇报这一轮：汇报交给 A，链合上。
  await runtime.forwardBotReplies(
    B,
    {},
    {
      message: null,
      completedAgents: ['11111111-2222-3333-4444-555555555555'],
      reply: '回归跑完了：3 个失败，都在退款模块。',
    },
  )
  assert.equal(runtime.delivered[2].botId, A.id)
  assert.match(runtime.delivered[2].text, /^回归跑完了：3 个失败/)
  assert.equal(parseBotMessageTag(runtime.delivered[2].text).hop, 3)
  assert.equal(runtime.botLinks.get(sent.chain).status, 'answered')
})

test('polite back-and-forth ends: courtesy is not sent, and a chain stops after 6 hops', async () => {
  const runtime = twoBots()
  assert.deepEqual(
    await runtime.executeBotDecision('conv-a', {}, A, {
      type: 'message',
      bot: '测试',
      text: '谢谢！',
      expectsReply: true,
    }),
    { type: 'message', skipped: 'courtesy' },
  )
  assert.equal(runtime.delivered.length, 0)
  // B 回了一句客气话：不交回去。
  await runtime.forwardBotReplies(
    B,
    {},
    {
      message: { from: A.id, chain: 'ch_abcdefghij', hop: 1, reply: true },
      completedAgents: [],
      reply: '好的',
    },
  )
  assert.equal(runtime.delivered.length, 0)
  // 链已经走到第 6 跳：再回一句就停下，两边时间线写着停在这里了。
  const result = await runtime.executeBotDecision(
    'conv-a',
    {},
    A,
    { type: 'message', bot: '测试', text: '那再跑一遍吧', expectsReply: true },
    { message: { from: B.id, chain: 'ch_abcdefghij', hop: MAX_BOT_LINK_HOPS, reply: false } },
  )
  assert.equal(result.skipped, 'hops')
  assert.equal(runtime.delivered.length, 0)
  assert.equal(runtime.botLinks.get('ch_abcdefghij').status, 'stopped')
  assert.equal(runtime.botLinks.listForBot(B.id)[0].status, 'stopped')
  // 不能找自己，也不能找不存在或暂停的 Bot。
  await assert.rejects(
    runtime.executeBotDecision('conv-a', {}, A, { type: 'message', bot: '小橙', text: 'x' }),
    /no such bot|bot not found/i,
  )
  await assert.rejects(
    runtime.executeBotDecision('conv-a', {}, A, { type: 'message', bot: '没有这个', text: 'x' }),
    /no such bot|bot not found/i,
  )
})

test('a message to a third bot keeps counting hops, so a ring of bots still stops at 6', async () => {
  const C = { id: 'bot_cccccccccccc', name: '发布', enabled: true, tools: null, instructions: 'CI' }
  const runtime = twoBots()
  runtime.bots = {
    list: () => [A, B, C],
    get: (id) => [A, B, C].find((bot) => bot.id === id) || null,
  }
  // B 这一轮是 A 说进来的话（第 2 跳），它转头找 C：新的一条来往，但跳数接着数。
  const sent = await runtime.executeBotDecision(
    'conv-b',
    {},
    B,
    { type: 'message', bot: '发布', text: '帮我看下 CI 为什么红了', expectsReply: true },
    { message: { from: A.id, chain: 'ch_ringringri', hop: 2, reply: true }, completedAgents: [] },
  )
  assert.notEqual(sent.chain, 'ch_ringringri')
  assert.equal(parseBotMessageTag(runtime.delivered[0].text).hop, 3)
  // 替别的 Bot 做的活做完后的那一轮同样接着数。
  runtime.botAgentRequests = new Map([
    ['11111111-2222-3333-4444-555555555555', { fromBotId: A.id, chain: 'ch_ringringri', hop: 4 }],
  ])
  await runtime.executeBotDecision(
    'conv-b',
    {},
    B,
    { type: 'message', bot: '发布', text: '回归里有 3 个失败和 CI 有关', expectsReply: false },
    { message: null, completedAgents: ['11111111-2222-3333-4444-555555555555'] },
  )
  assert.equal(parseBotMessageTag(runtime.delivered[1].text).hop, 5)
  // 已经在第 6 跳：再找第三个 Bot 就停下。
  const stopped = await runtime.executeBotDecision(
    'conv-b',
    {},
    B,
    { type: 'message', bot: '发布', text: '那再跑一遍吧', expectsReply: true },
    {
      message: { from: A.id, chain: 'ch_ringringri', hop: MAX_BOT_LINK_HOPS, reply: true },
      completedAgents: [],
    },
  )
  assert.equal(stopped.skipped, 'hops')
  assert.equal(runtime.delivered.length, 2)
  // 你说的话开头的一轮还是第 1 跳。
  await runtime.executeBotDecision(
    'conv-b',
    {},
    B,
    { type: 'message', bot: '发布', text: '帮我确认一下发布时间', expectsReply: true },
    { message: null, completedAgents: [] },
  )
  assert.equal(parseBotMessageTag(runtime.delivered[2].text).hop, 1)
})

// messageBot 用得到的那几样：会话空闲，promptFromChannel 由每条测试自己给。
function messagingRuntime() {
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  const notified = []
  Object.assign(runtime, {
    notified,
    bots: { list: () => [A, B], get: (id) => [A, B].find((bot) => bot.id === id) || null },
    ensureBotSession: async () => 'conv-a',
    sessionRunIsActive: () => false,
    startingRuns: new Map(),
    sessions: new Map(),
    botChatOrigins: new Map(),
    botPendingTrust: new Map(),
    notifyBotChat: (source, text) => notified.push({ source, text }),
  })
  return runtime
}

test('a schedule learns whether the bot really started; a failed start is an error, not “sent”', async () => {
  const runtime = messagingRuntime()
  const finish = Promise.withResolvers()
  runtime.promptFromChannel = async ({ onEvent }) => {
    onEvent('meta', {})
    await finish.promise
    return { text: '做完了', error: '' }
  }
  // 这一轮开始就返回，不等它说完。
  assert.deepEqual(
    await runtime.messageBot(A.id, {
      text: '查依赖',
      source: { kind: 'schedule' },
      waitForStart: true,
    }),
    { sessionId: 'conv-a', status: 'started' },
  )
  finish.resolve()

  runtime.promptFromChannel = async () => {
    throw new Error('Model is unavailable')
  }
  await assert.rejects(
    runtime.messageBot(A.id, { text: '查依赖', source: { kind: 'schedule' }, waitForStart: true }),
    /Model is unavailable/,
  )
  // 运行出错不抛、只在结果里带 error 的那种也算没做成。
  runtime.promptFromChannel = async () => ({ text: '', error: 'Session is busy' })
  await assert.rejects(
    runtime.messageBot(A.id, { text: '查依赖', source: { kind: 'schedule' }, waitForStart: true }),
    /Session is busy/,
  )
  // 不要求等的调用方（聊天、别的 Bot）照旧立刻返回。
  runtime.promptFromChannel = () => new Promise(() => {})
  assert.equal((await runtime.messageBot(A.id, { text: '在吗' })).status, 'started')
})

test('trust waiting for a turn is never loosened by a later source', async () => {
  const runtime = messagingRuntime()
  runtime.promptFromChannel = () => new Promise(() => {})
  await runtime.messageBot(A.id, {
    text: '群里的人说的',
    source: { kind: 'channel', platform: 'feishu', peerId: 'g1' },
  })
  assert.equal(runtime.botPendingTrust.get('conv-a'), 'external')
  // 还没被这一轮取走时你又说了一句：不能把上一句变成“用户说的”。
  await runtime.messageBot(A.id, { text: '我说的', source: { kind: 'chat' } })
  assert.equal(runtime.botPendingTrust.get('conv-a'), 'external')
  assert.equal(runtime.takeBotTurnTrust('conv-a'), 'external')
  await runtime.messageBot(A.id, { text: '我说的', source: { kind: 'chat' } })
  assert.equal(runtime.takeBotTurnTrust('conv-a'), 'user')
})

test('a turn that was stopped or failed tells the waiting chats and forgets them', () => {
  const runtime = messagingRuntime()
  runtime.botForSession = () => A
  const group = { kind: 'channel', platform: 'feishu', peerId: 'g1' }
  const value = { pendingChatReplies: [group] }
  runtime.dropBotChatReplies('conv-a', value)
  assert.deepEqual(value.pendingChatReplies, [])
  assert.equal(runtime.notified.length, 1)
  assert.equal(runtime.notified[0].source, group)
  assert.match(runtime.notified[0].text, /小橙/)
})

test('the bots list does not project job records for a bot with nothing running', () => {
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  let projected = 0
  Object.assign(runtime, {
    sessionRunIsActive: () => false,
    startingRuns: new Map(),
    botCommands: { list: () => [] },
    permissions: { getPending: () => [] },
    questions: { getPending: () => [] },
    multiAgents: {
      hasActive: () => runtime.active,
      summaries: () => {
        projected += 1
        return [{ id: 'j1', status: 'running', title: '跑回归' }]
      },
    },
  })
  runtime.active = false
  assert.deepEqual(runtime.botActivity({ ...A, threadSessionId: 'conv-a' }), {
    status: 'idle',
    waiting: null,
    needsYou: false,
    working: 0,
    current: '',
  })
  assert.equal(projected, 0)
  runtime.active = true
  const busy = runtime.botActivity({ ...A, threadSessionId: 'conv-a' })
  assert.deepEqual([busy.status, busy.working, busy.current], ['running', 1, '跑回归'])
})
