// Bot 主动（C6，默认关）：空闲时只读巡看、每日简报。巡看子 agent 只有读文件、搜索、列目录的工具，
// 写操作根本不在它手上；巡看结果与简报那一轮不开活（只能记、忘）；关掉后不再巡看，正在跑的巡看也停下。
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { botTurnTrigger } from '../runtime/bot-decisions.mjs'
import { isInternalParentMessage } from '../runtime/stream-projection.mjs'
import { normalizeBotInput } from '../services/bots/bot-model.mjs'
import {
  BotProactiveStore,
  botBriefPrompt,
  botProactiveDue,
  botProactiveTools,
} from '../services/bots/bot-proactive.mjs'
import { agentCompletionPrompt } from '../services/multi-agent-service.mjs'

const HOUR = 60 * 60 * 1000

test('proactive is off by default; settings merge and fall back to safe values', () => {
  const bot = normalizeBotInput({ name: '巡检', workspace: { cwd: '/tmp' } })
  assert.deepEqual(bot.proactive, { enabled: false, everyHours: 6, brief: false, briefAt: '09:00' })
  assert.equal(bot.maxParallelAgents, 3)
  const changed = normalizeBotInput({ proactive: { enabled: true }, maxParallelAgents: 5 }, bot)
  assert.deepEqual(changed.proactive, {
    enabled: true,
    everyHours: 6,
    brief: false,
    briefAt: '09:00',
  })
  assert.equal(changed.maxParallelAgents, 5)
  const odd = normalizeBotInput(
    { proactive: { everyHours: 7, briefAt: '25:00', brief: 'yes' }, maxParallelAgents: 99 },
    changed,
  )
  assert.deepEqual(odd.proactive, { enabled: true, everyHours: 6, brief: false, briefAt: '09:00' })
  assert.equal(odd.maxParallelAgents, 3)
})

test('checks are due every few hours, briefs once a day after their time', () => {
  const bot = { proactive: { enabled: true, everyHours: 3, brief: true, briefAt: '09:00' } }
  const morning = new Date(2026, 10, 4, 9, 30).getTime()
  assert.equal(botProactiveDue(bot, {}, morning).check, true)
  assert.equal(
    botProactiveDue(bot, { lastCheckAt: new Date(morning - 2 * HOUR).toISOString() }, morning)
      .check,
    false,
  )
  assert.equal(
    botProactiveDue(bot, { lastCheckAt: new Date(morning - 3 * HOUR).toISOString() }, morning)
      .check,
    true,
  )
  const due = botProactiveDue(bot, {}, morning)
  assert.equal(due.brief, true)
  assert.equal(botProactiveDue(bot, { lastBriefDay: due.day }, morning).brief, false)
  assert.equal(botProactiveDue(bot, {}, new Date(2026, 10, 4, 8, 59).getTime()).brief, false)
  assert.deepEqual(botProactiveDue({ proactive: { enabled: false } }, {}, morning), {
    check: false,
    brief: false,
    day: due.day,
  })
})

test('the check only ever gets read-only tools', () => {
  assert.deepEqual(botProactiveTools({ tools: null }), ['read', 'grep', 'find', 'ls'])
  assert.deepEqual(botProactiveTools({ tools: ['read', 'bash', 'edit', 'write'] }), ['read'])
  assert.deepEqual(botProactiveTools({ tools: ['bash'] }), [])
})

test('the brief is a hidden message and its turn, like a check’s result, starts no work', () => {
  const brief = botBriefPrompt({
    bot: { name: '巡检' },
    jobs: [{ title: '依赖巡检', status: 'completed' }],
    schedules: ['09:00 看一遍 CI'],
  })
  assert.equal(isInternalParentMessage(brief), true)
  const reply = { role: 'assistant', content: [{ type: 'text', text: '今天的简报：…' }] }
  assert.equal(
    botTurnTrigger([{ role: 'user', content: [{ type: 'text', text: brief }] }, reply]).brief,
    true,
  )
})

const BOT = {
  id: 'bot_aaaaaaaaaaaa',
  name: '巡检',
  enabled: true,
  instructions: '看依赖与 CI',
  tools: null,
  threadSessionId: 'conv',
  workspace: { environmentId: 'local', cwd: '/tmp' },
  maxParallelAgents: 3,
  proactive: { enabled: true, everyHours: 6, brief: false, briefAt: '09:00' },
}

async function proactiveRuntime(dataDir, { busy = false, flag = true, decisions = [] } = {}) {
  const spawned = []
  const calls = []
  const agents = []
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  const value = {
    conversationOnly: true,
    baseToolNames: ['read', 'grep', 'find', 'ls', 'edit', 'write', 'bash'],
    session: { model: { provider: 'luna', id: 'm' }, messages: [], systemPrompt: '' },
    multiAgentRuntime: {
      spawn: async (input) => {
        spawned.push(input)
        const agent = {
          id: '11111111-2222-3333-4444-55555555555' + spawned.length,
          status: 'running',
          title: input.title,
        }
        agents.push(agent)
        return agent
      },
    },
  }
  Object.assign(runtime, {
    spawned,
    calls,
    agents,
    value,
    capabilities: { features: { bots: true } },
    isBetaFeatureEnabled: () => flag,
    bots: { list: () => [BOT], get: () => BOT },
    botForSession: () => BOT,
    botProactive: new BotProactiveStore({ dataDir }),
    botMemory: { list: async () => [], read: async () => ({ entries: [], tidiedAt: null }) },
    sessions: new Map([['conv', value]]),
    startingRuns: new Set(),
    sessionRunIsActive: () => busy,
    getOrCreateSession: async () => value,
    permissions: { getPending: () => [] },
    multiAgents: {
      list: () => agents,
      hasActive: () => agents.some((agent) => agent.status === 'running'),
      interrupt: (parent, id) => calls.push(['interrupt', parent, id]),
      followup: async () => {},
    },
    providerPreferences: {
      availableChatModelRefs: async () => [],
      resolveSubagentModel: async () => ({ model: null }),
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
    executeBotDecision: async (_session, _value, _bot, decision) => {
      calls.push(['decision', decision.type])
      return decision
    },
  })
  return runtime
}

async function withDir(run) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-bot-proactive-'))
  try {
    await run(dataDir)
  } finally {
    await rm(dataDir, { recursive: true, force: true })
  }
}

test('an idle bot with checks on gets a read-only check, then waits for the next interval', async () => {
  await withDir(async (dataDir) => {
    const runtime = await proactiveRuntime(dataDir)
    const now = Date.now()
    const started = await runtime.botProactiveTick(now)
    assert.equal(started[0].kind, 'check')
    assert.deepEqual(runtime.spawned[0].toolNames, ['read', 'grep', 'find', 'ls'])
    assert.equal(runtime.spawned[0].title, '巡看')
    assert.match(runtime.spawned[0].message, /只读/)
    assert.match(
      runtime.botCompletionPromptFor({ ...runtime.agents[0], output: '没什么问题' }),
      /read-only check; nobody asked/,
    )
    // 手上有活（巡看还在跑）、或者刚巡看过：都不再开。
    runtime.agents[0].status = 'completed'
    assert.deepEqual(await runtime.botProactiveTick(now + HOUR), [])
    assert.equal(runtime.spawned.length, 1)
    assert.equal((await runtime.botProactiveTick(now + 6 * HOUR))[0].kind, 'check')
  })
})

test('busy bots, bots with checks off and the bots flag off are never checked', async () => {
  await withDir(async (dataDir) => {
    assert.deepEqual(await (await proactiveRuntime(dataDir, { busy: true })).botProactiveTick(), [])
    assert.deepEqual(
      await (await proactiveRuntime(dataDir, { flag: false })).botProactiveTick(),
      [],
    )
    const off = await proactiveRuntime(dataDir)
    off.bots = { list: () => [{ ...BOT, proactive: { ...BOT.proactive, enabled: false } }] }
    assert.deepEqual(await off.botProactiveTick(), [])
    assert.equal(off.spawned.length, 0)
  })
})

test('a check’s result turn may only remember: no work starts until the user says so', async () => {
  await withDir(async (dataDir) => {
    const runtime = await proactiveRuntime(dataDir, {
      decisions: [
        { type: 'spawn', title: '升级 undici', task: '升级 undici 并跑测试' },
        { type: 'message', bot: '测试', text: '帮我跑测试' },
        { type: 'remember', text: 'undici 6.21.1 有中危更新', source: 'proactive' },
      ],
    })
    await runtime.botProactiveTick()
    const agent = runtime.agents[0]
    agent.status = 'completed'
    runtime.value.session.messages = [
      {
        role: 'user',
        content: [{ type: 'text', text: runtime.botCompletionPromptFor(agent) }],
      },
      {
        role: 'assistant',
        content: [{ type: 'text', text: '巡看发现 undici 有安全更新，要升级吗？' }],
      },
    ]
    runtime.forwardBotReplies = async () => []
    runtime.tidyBotMemory = async () => null
    await runtime.runBotDecisions('conv')
    assert.deepEqual(
      runtime.calls.filter(([kind]) => kind === 'decision'),
      [['decision', 'remember']],
    )
    // 之后用户说“做吧”的那一轮照常能派活。
    runtime.value.session.messages.push(
      { role: 'user', content: [{ type: 'text', text: '做吧' }] },
      { role: 'assistant', content: [{ type: 'text', text: '好，我派人去升级。' }] },
    )
    await runtime.runBotDecisions('conv')
    assert.deepEqual(runtime.calls.filter(([kind]) => kind === 'decision').slice(1), [
      ['decision', 'spawn'],
      ['decision', 'message'],
      ['decision', 'remember'],
    ])
  })
})

test('turning checks off stops a running check', async () => {
  await withDir(async (dataDir) => {
    const runtime = await proactiveRuntime(dataDir)
    await runtime.botProactiveTick()
    const off = { ...BOT, proactive: { ...BOT.proactive, enabled: false } }
    runtime.bots = {
      get: () => BOT,
      update: async () => off,
      list: () => [off],
    }
    runtime.applyBotSettings = async () => {}
    await runtime.updateBot(BOT.id, { proactive: { enabled: false } })
    assert.deepEqual(runtime.calls, [['interrupt', 'conv', runtime.agents[0].id]])
    assert.deepEqual(await runtime.botProactiveTick(Date.now() + 7 * HOUR), [])
  })
})

test('the completion prompt for ordinary jobs is unchanged', () => {
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  assert.equal(runtime.botCompletionPromptFor({ id: 'x' }), null)
  assert.match(agentCompletionPrompt({ id: 'x', taskName: 't', status: 'completed' }), /completed/)
})
