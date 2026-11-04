// Bot 的对话（v2）：对话只说话、做决定，活派给子 agent。决定的解析、状态快照、对话的系统提示，
// 以及运行时按决定派、补充、停子 agent，停掉对话不停子 agent，每个 Bot 同时最多 3 个。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { botDecisionContext } from '../runtime/bot-decisions.mjs'
import {
  BOT_MAX_PARALLEL_AGENTS,
  botConversationPrompt,
  botDecisionInstruction,
  botStateSnapshot,
  parseBotDecisions,
  resolveBotAgent,
} from '../services/bots/bot-conversation.mjs'
import { MultiAgentService } from '../services/multi-agent-service.mjs'

const agent = (id, taskName, status, output = '') => ({ id, taskName, status, output, error: '' })

test('decisions are parsed strictly; anything malformed is dropped', () => {
  const raw = `好的。\n{"actions":[
    {"type":"spawn","title":"补退款测试","task":"在 src/checkout 里补退款与部分退款的测试，跑一遍后汇报","model":"luna/gpt-5.6-luna","tier":"strong"},
    {"type":"spawn","title":"","task":"没有标题"},
    {"type":"add","agent":"a1b2","text":"边界值也测一下"},
    {"type":"stop","agent":"c3d4"},
    {"type":"delete_everything"},
    {"type":"spawn","title":"x","task":"y","tier":"huge","model":"bare"}
  ]}`
  assert.deepEqual(parseBotDecisions(raw), [
    {
      type: 'spawn',
      title: '补退款测试',
      task: '在 src/checkout 里补退款与部分退款的测试，跑一遍后汇报',
      model: 'luna/gpt-5.6-luna',
      tier: 'strong',
    },
    { type: 'add', agent: 'a1b2', text: '边界值也测一下' },
    { type: 'stop', agent: 'c3d4' },
    { type: 'spawn', title: 'x', task: 'y' },
  ])
  assert.deepEqual(parseBotDecisions('没有 JSON'), [])
  assert.deepEqual(parseBotDecisions('{"actions": "nope"}'), [])
  assert.deepEqual(parseBotDecisions('{broken'), [])
})

test('the state snapshot and the decision instruction list what the bot has in hand', () => {
  const agents = [
    agent('a1b2c3d4-1', '补退款测试', 'running', '写完了退款用例，正在跑'),
    agent('e5f6a7b8-2', '依赖巡检', 'completed', '没有安全更新'),
  ]
  const snapshot = botStateSnapshot({ agents, waitingApprovals: 1 })
  assert.match(snapshot, /^\[当前状态\]/)
  assert.match(snapshot, /现在在做 1 个，1 个操作在等用户批准/)
  assert.match(snapshot, /- \[a1b2c3d4\] 补退款测试 · running · 写完了退款用例，正在跑/)
  assert.match(snapshot, /最近结束：\n- \[e5f6a7b8\] 依赖巡检 · completed/)
  assert.match(botStateSnapshot(), /手上没有活/)
  const instruction = botDecisionInstruction({ agents, models: ['luna/gpt-5.6-luna'] })
  assert.match(instruction, /Running sub-agents: a1b2c3d4 "补退款测试"/)
  assert.match(instruction, /Available models: luna\/gpt-5.6-luna/)
  assert.match(instruction, new RegExp(`At most ${BOT_MAX_PARALLEL_AGENTS} sub-agents`))
  assert.equal(resolveBotAgent(agents, 'a1b2')?.id, 'a1b2c3d4-1')
  assert.equal(resolveBotAgent(agents, ''), null)
  assert.equal(resolveBotAgent(agents, 'zz'), null)
})

test('the conversation prompt makes the bot a manager that does not run tools itself', () => {
  const prompt = botConversationPrompt({ name: '小橙', instructions: '负责 acme-web 前端' })
  assert.match(prompt, /你是 小橙/)
  assert.match(prompt, /你自己不能调用任何工具/)
  assert.match(prompt, /派子 agent/)
  assert.match(prompt, /\[小橙 的工作说明\]\n负责 acme-web 前端/)
  const context = botDecisionContext(
    { systemPrompt: 'sys', messages: [{ role: 'user', content: 'hi', timestamp: 1 }] },
    'decide',
    2,
  )
  assert.equal(context.systemPrompt, 'sys')
  assert.deepEqual(context.tools, [])
  assert.equal(context.messages.at(-1).content[0].text, 'decide')
})

// 只装决定执行用得到的那几样。
function fakeRuntime({ decisions, agents = [] }) {
  const calls = []
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  const bot = { id: 'bot1', name: '小橙', tools: null, threadSessionId: 'conv' }
  Object.assign(runtime, {
    calls,
    sessions: new Map([
      [
        'conv',
        {
          conversationOnly: true,
          baseToolNames: ['read', 'edit', 'bash'],
          session: { model: { provider: 'luna', id: 'm' }, messages: [], systemPrompt: '' },
          multiAgentRuntime: {
            spawn: async (input) => {
              calls.push(['spawn', input])
              return { id: `new-${calls.length}` }
            },
          },
        },
      ],
    ]),
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
    modelRuntime: {
      streamSimple: () => ({
        async *[Symbol.asyncIterator]() {
          yield { type: 'text_delta', delta: JSON.stringify({ actions: decisions }) }
        },
        result: async () => ({ stopReason: 'stop', usage: null }),
      }),
    },
    recordUsage: async () => {},
    // 记忆：派活时带上；这里没有记忆，也不整理。
    botMemory: { list: async () => [], read: async () => ({ entries: [], tidiedAt: null }) },
  })
  return runtime
}

test('the runtime spawns, adds to and stops only its own sub-agents as decided', async () => {
  const runtime = fakeRuntime({
    agents: [agent('a1b2c3d4', '补退款测试', 'running')],
    decisions: [
      {
        type: 'spawn',
        title: '写更新说明',
        task: '把这周的改动整理成更新说明',
        model: 'nova/coder',
      },
      { type: 'spawn', title: '跑回归', task: '跑一遍回归测试', tier: 'fast' },
      { type: 'add', agent: 'a1b2', text: '边界值也测一下' },
      { type: 'stop', agent: 'a1b2' },
      { type: 'stop', agent: 'not-mine' },
    ],
  })
  const done = await runtime.runBotDecisions('conv')
  assert.deepEqual(
    done.map((item) => item.type),
    ['spawn', 'spawn', 'add', 'stop'],
  )
  const [first, second] = runtime.calls
    .filter(([name]) => name === 'spawn')
    .map(([, input]) => input)
  // 工具按 Bot 的范围直接给（null = 全部），每个 Bot 同时最多 3 个，模型按活挑。
  assert.deepEqual(first.toolNames, ['read', 'edit', 'bash'])
  assert.equal(first.parentLimit, BOT_MAX_PARALLEL_AGENTS)
  assert.deepEqual(first.resolvedModel, { provider: 'nova', id: 'coder' })
  assert.equal(first.taskName, '写更新说明')
  assert.equal(second.tier, 'fast')
  assert.equal('resolvedModel' in second, false)
  assert.deepEqual(
    runtime.calls.find(([name]) => name === 'followup'),
    ['followup', 'conv', 'a1b2c3d4', '边界值也测一下'],
  )
  assert.deepEqual(
    runtime.calls.find(([name]) => name === 'interrupt'),
    ['interrupt', 'conv', 'a1b2c3d4'],
  )
  // 不是 Bot 的对话：什么都不做。
  assert.deepEqual(await runtime.runBotDecisions('other'), [])
})

test('decisions for one conversation run one after another', async () => {
  const runtime = fakeRuntime({ decisions: [] })
  const order = []
  runtime.runBotDecisions = async (id) => {
    order.push(`start-${order.length}`)
    await new Promise((resolve) => setTimeout(resolve, 5))
    order.push(`end-${id}`)
  }
  await Promise.all([runtime.scheduleBotDecisions('conv'), runtime.scheduleBotDecisions('conv')])
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

test('the runtime wires the bot conversation: no tools, state per turn, decisions after, stop spares sub-agents', async () => {
  const read = (path) => readFile(new URL(path, import.meta.url), 'utf8')
  const [runtime, activation, lifecycle, facade] = await Promise.all([
    read('../runtime/agent-runtime.mjs'),
    read('../runtime/tool-activation.mjs'),
    read('../runtime/session-lifecycle.mjs'),
    read('../runtime/agent-runtime-facade.mjs'),
  ])
  assert.match(
    activation,
    /if \(value\.conversationOnly\) \{\s*value\.session\.setActiveToolsByName\(\[\]\)/,
  )
  assert.match(
    runtime,
    /if \(value\.conversationOnly\) contexts\.push\(await this\.botStateContext\(session\.sessionId\)\)/,
  )
  assert.match(
    runtime,
    /if \(value\.conversationOnly && !error && !value\.abortedAt\) this\.scheduleBotDecisions\(sessionId\)/,
  )
  assert.match(
    lifecycle,
    /if \(!value\?\.conversationOnly\) \{\s*this\.getMultiAgents\(\)\.abortParent\(id\)/,
  )
  assert.match(
    facade,
    /if \(!this\.multiAgents\?\.hasActive\?\.\(sessionId\)\)\s*this\.disposeSessionRuntime/,
  )
})
