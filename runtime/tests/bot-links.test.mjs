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
  assert.match(context, /来自 Bot「小橙」，不是用户/)
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
    /不存在/,
  )
  await assert.rejects(
    runtime.executeBotDecision('conv-a', {}, A, { type: 'message', bot: '没有这个', text: 'x' }),
    /不存在/,
  )
})
