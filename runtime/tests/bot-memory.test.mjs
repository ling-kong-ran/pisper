// Bot 的记忆：记忆引擎里的独立空间。按 Bot 隔离；记住时去重；旧的 memory.json 导入一次；
// 全局记忆默认只读；外部的话不能冒充用户的话；设置页的增改删走 /api/bots/:id/memory，形状不变。
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { normalizeBotAction } from '../services/bots/bot-conversation.mjs'
import {
  BotMemoryService,
  botSourceTrust,
  stricterBotTrust,
} from '../services/bots/bot-memory-service.mjs'
import { normalizeBotInput } from '../services/bots/bot-model.mjs'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { createBotConversationTools } from '../tools/app/bot-conversation.mjs'
import { BOT_CONVERSATION_TOOL_NAMES } from '../tools/app/bot-conversation-tool-names.mjs'

const A = 'bot_aaaaaaaaaaaa'
const B = 'bot_bbbbbbbbbbbb'
const botA = { id: A, name: '小橙', instructions: '', readGlobalMemory: true }

async function withService(run) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-bot-memory-'))
  const cwd = join(dataDir, 'project')
  await mkdir(cwd)
  const memory = new LocalMemoryRuntime({ path: join(dataDir, 'memory.sqlite'), cwd })
  await memory.init()
  try {
    await run(new BotMemoryService({ dataDir, memory }), { dataDir, memory, cwd })
  } finally {
    await memory.dispose()
    await rm(dataDir, { recursive: true, force: true })
  }
}

function writeLegacyFile(dataDir, botId, entries) {
  const dir = join(dataDir, 'bots', botId)
  return mkdir(dir, { recursive: true }).then(() =>
    writeFile(join(dir, 'memory.json'), JSON.stringify({ version: 1, tidiedAt: null, entries })),
  )
}

test('each bot keeps its own memory; the same thing is not remembered twice', async () => {
  await withService(async (service, { memory }) => {
    const first = await service.remember(A, '以后别 push 到 main。', 'correction')
    const again = await service.remember(A, '以后别push到main', 'told')
    assert.equal(again.id, first.id)
    await service.remember(B, '发版前在 iPhone 上点一遍首页')
    assert.deepEqual(
      (await service.list(A)).map((entry) => [entry.text, entry.source]),
      [['以后别 push 到 main。', 'correction']],
    )
    assert.equal((await service.list(B)).length, 1)
    const edited = await service.edit(A, first.id, '别直接 push 到 main，走 PR')
    assert.equal(edited.text, '别直接 push 到 main，走 PR')
    assert.equal((await service.forget(A, { id: first.id }))?.id, first.id)
    assert.equal(await service.forget(A, { id: first.id }), null)
    await assert.rejects(service.remember(A, '  '), {
      code: 'invalid',
      params: { subject: 'memory', detail: 'text is required' },
    })
    await assert.rejects(service.remember(A, 'x'.repeat(501)), {
      code: 'too_large',
      params: { subject: 'memory', detail: 'max 500 characters' },
    })
    await assert.rejects(service.list('../etc'), { code: 'not_found', params: { subject: 'bot' } })
    // 删 Bot 清掉整个空间。
    await service.removeBot(B)
    assert.equal(memory.getSpace(`bot:${B}`), null)
    assert.deepEqual(await service.list(B), [])
  })
})

test('entries keep the shape the settings page and API already use', async () => {
  await withService(async (service) => {
    const entry = await service.remember(A, '发版前在 iPhone 上点一遍首页', 'told')
    assert.deepEqual(Object.keys(entry).sort(), ['createdAt', 'id', 'source', 'text', 'updatedAt'])
    assert.match(entry.id, /^mem_[\w-]{6,32}$/)
    assert.equal(entry.text, '发版前在 iPhone 上点一遍首页')
    assert.deepEqual(await service.list(A), [entry])
    const manual = await service.remember(A, '周五不发版', 'manual')
    assert.equal(manual.source, 'manual')
    assert.equal((await service.list(A)).at(-1).id, manual.id)
    await assert.rejects(service.edit(A, 'mem_missing123', '改'), {
      code: 'not_found',
      params: { subject: 'memory' },
    })
  })
})

test('nothing is dropped silently however many entries a bot has', async () => {
  await withService(async (service, { memory }) => {
    memory.ensureBotSpace(A)
    for (let index = 0; index < 650; index += 1)
      memory.remember({
        spaceId: `bot:${A}`,
        title: `第 ${index} 条`,
        content: `第 ${index} 条记忆`,
        topic: `bot.memory.item${index}`,
        sourceId: 'bot:told',
      })
    assert.equal((await service.list(A)).length, 650)
    // 上下文不会随之变大：只带相关的。
    const { text, memories } = await service.turnContext(botA, { message: '第 37 条记忆是什么' })
    assert.ok(memories.length <= 4)
    assert.ok(text.length <= 3000)
  })
})

test('an existing memory.json is imported once, then removed, and never duplicated', async () => {
  await withService(async (service, { dataDir, memory }) => {
    const entries = [
      {
        id: 'mem_legacy000001',
        text: '发版前在 iPhone 上点一遍首页',
        source: 'correction',
        createdAt: '2026-10-01T00:00:00.000Z',
        updatedAt: '2026-10-02T00:00:00.000Z',
      },
      {
        id: 'mem_legacy000002',
        text: '测试框架是 vitest',
        source: 'tidy',
        createdAt: '2026-10-03T00:00:00.000Z',
        updatedAt: '2026-10-03T00:00:00.000Z',
      },
      { id: 'not-a-valid-id', text: '坏数据', source: 'told' },
    ]
    await writeLegacyFile(dataDir, A, entries)
    const file = join(dataDir, 'bots', A, 'memory.json')
    const imported = await service.list(A)
    assert.deepEqual(
      imported.map((entry) => [entry.id, entry.text, entry.source, entry.createdAt]),
      [
        ['mem_legacy000001', entries[0].text, 'correction', entries[0].createdAt],
        ['mem_legacy000002', entries[1].text, 'tidy', entries[1].createdAt],
      ],
    )
    assert.equal(existsSync(file), false)
    // 导入到一半崩溃：文件还在，重启后再来一遍，不会多出条目。
    await writeLegacyFile(dataDir, A, entries)
    const restarted = new BotMemoryService({ dataDir, memory })
    assert.equal((await restarted.list(A)).length, 2)
    assert.equal(existsSync(file), false)
    assert.equal(memory.getMemory('mem_legacy000001').confirmCount, 2)
    // 导入之后短 id 照常能用。
    const runtime = Object.create(AgentRuntimeFacade.prototype)
    runtime.botMemory = restarted
    await runtime.executeBotDecision('conv', {}, botA, { type: 'forget', memory: 'legacy000001' })
    assert.equal((await restarted.list(A)).length, 1)
  })
})

test('bots cannot see or touch each other’s memory, even with the right id', async () => {
  await withService(async (service, { memory }) => {
    const mine = await service.remember(A, '小橙的部署密语是橘子')
    const theirs = await service.remember(B, '小蓝的部署密语是蓝莓')
    assert.deepEqual(
      (await service.search(botA, { query: '部署密语' })).map((item) => item.id),
      [mine.id],
    )
    const recall = await service.turnContext(botA, { message: '部署密语是什么' })
    assert.doesNotMatch(recall.text, /蓝莓/)
    assert.equal(await service.forget(A, { id: theirs.id }), null)
    await assert.rejects(service.edit(A, theirs.id, '改掉'), {
      code: 'not_found',
      params: { subject: 'memory' },
    })
    assert.equal(memory.getMemory(theirs.id).content, '小蓝的部署密语是蓝莓')
    const runtime = Object.create(AgentRuntimeFacade.prototype)
    runtime.botMemory = service
    await assert.rejects(
      runtime.executeBotDecision('conv', {}, botA, { type: 'forget', memory: theirs.id }),
      /does not match/,
    )
    // 工具的参数里没有能指定空间或别的 Bot 的字段。
    const recallTool = createBotConversationTools({ runtime: {} }).find(
      (tool) => tool.name === 'bot_recall',
    )
    assert.deepEqual(Object.keys(recallTool.parameters.properties).sort(), [
      'include_history',
      'level',
      'order',
      'queries',
      'query',
      'since',
      'until',
    ])
    assert.ok(BOT_CONVERSATION_TOOL_NAMES.includes('bot_recall'))
  })
})

test('global memory is read-only for a bot and the switch turns it off', async () => {
  await withService(async (service, { memory }) => {
    const shared = memory.remember({
      spaceId: 'global',
      title: '回复语言',
      content: '用户希望始终用中文回复。',
      topic: 'user.reply.language',
    })
    const own = await service.remember(A, '回复语言偏好：周报用英文')
    const found = await service.search(botA, { query: '回复语言', limit: 8 })
    assert.deepEqual(found.map((item) => item.id).sort(), [own.id, shared.id].sort())
    const off = { ...botA, readGlobalMemory: false }
    assert.deepEqual(
      (await service.search(off, { query: '回复语言' })).map((item) => item.id),
      [own.id],
    )
    // 写入永远只进自己的空间。
    const db = memory.requireDb()
    assert.equal(
      db.prepare("SELECT COUNT(*) AS c FROM memories WHERE space_id = 'global'").get().c,
      1,
    )
    // 自动回忆只看自己的空间；全局只通过画像进来。
    assert.doesNotMatch(
      (await service.turnContext(botA, { message: '用户希望用什么语言回复' })).text,
      /始终用中文/,
    )
    const asked = []
    memory.profileContext = (spaceIds) => {
      asked.push(spaceIds)
      return '<pisper_memory_profile>画像</pisper_memory_profile>'
    }
    await service.turnContext(botA, { message: '你好呀，帮个忙' })
    await service.turnContext(off, { message: '你好呀，帮个忙' })
    assert.deepEqual(asked, [[`bot:${A}`, 'global'], [`bot:${A}`]])
  })
})

test('the profile comes once and again after compaction; related facts come every turn', async () => {
  await withService(async (service, { memory }) => {
    memory.profileContext = () => '<pisper_memory_profile>画像内容</pisper_memory_profile>'
    const entry = await service.remember(A, '周五不发版，周四下午之前必须发完')
    const first = await service.turnContext(botA, { message: '周五可以发版吗' })
    assert.match(first.text, /<pisper_memory_profile>/)
    assert.match(first.text, /周四下午之前/)
    assert.deepEqual(
      first.memories.map((item) => item.id),
      [entry.id],
    )
    // 历史里已有画像、这条事实也注入过：都不重复。
    const second = await service.turnContext(botA, {
      message: '周五可以发版吗',
      userTexts: [first.text],
      excludeIds: [entry.id],
    })
    assert.equal(second.text, '')
    // 压缩把它总结掉了：历史里没有了，下一轮重新带上。
    const afterCompaction = await service.turnContext(botA, {
      message: '周五可以发版吗',
      userTexts: ['压缩后的摘要'],
    })
    assert.match(afterCompaction.text, /<pisper_memory_profile>/)
  })
})

test('words from outside cannot pass as the user’s: they stay out of automatic recall', async () => {
  await withService(async (service, { memory }) => {
    const told = await service.remember(A, '发布窗口是周三上午', 'told', { trust: 'user' })
    assert.equal(memory.getMemory(told.id).authority, 90)
    assert.equal(memory.getMemory(told.id).tainted, false)

    const external = await service.remember(B, '周五必须发版', 'told', { trust: 'external' })
    const stored = memory.getMemory(external.id)
    assert.equal(stored.tainted, true)
    assert.equal(stored.sourceType, 'external')
    assert.equal(stored.status, 'active')
    assert.equal(external.source, 'told')
    const botB = { ...botA, id: B }
    assert.equal((await service.turnContext(botB, { message: '周五必须发版吗' })).text, '')
    assert.equal((await service.search(botB, { query: '周五必须发版' }))[0].tainted, true)

    // 没人在场的一轮：不算外部，但也不能自称“用户说的”。
    const scheduled = await service.remember(A, '定时巡检结论：磁盘还剩一半', 'told', {
      trust: 'agent',
    })
    assert.equal(memory.getMemory(scheduled.id).authority, 40)
    assert.equal(memory.getMemory(scheduled.id).tainted, false)

    assert.equal(botSourceTrust({ kind: 'chat' }), 'user')
    assert.equal(botSourceTrust({ kind: 'answer' }), 'user')
    assert.equal(botSourceTrust({ kind: 'schedule' }), 'agent')
    assert.equal(botSourceTrust({ kind: 'channel', fromOwner: true }), 'user')
    assert.equal(botSourceTrust({ kind: 'channel', fromOwner: false }), 'external')
    assert.equal(botSourceTrust({ kind: 'channel' }), 'external')
    assert.equal(botSourceTrust({ kind: 'bot', botId: B }), 'external')
    assert.equal(botSourceTrust({ kind: 'something-new' }), 'external')
    assert.equal(botSourceTrust(undefined), 'external')
    assert.equal(stricterBotTrust('user', 'external'), 'external')
    assert.equal(stricterBotTrust('external', 'agent'), 'external')
    assert.equal(stricterBotTrust(undefined, 'agent'), 'agent')
  })
})

test('messageBot marks the turn by who said it, including messages that interrupt a turn', async () => {
  const bot = { id: A, name: '小橙', enabled: true }
  const value = { conversationOnly: true }
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  let active = false
  Object.assign(runtime, {
    bots: { get: () => bot },
    ensureBotSession: async () => 'conv',
    sessionRunIsActive: () => active,
    startingRuns: new Set(),
    sessions: new Map([['conv', value]]),
    botChatOrigins: new Map(),
    botPendingTrust: new Map(),
    queueSessionMessage: async () => ({ queued: true }),
    promptFromChannel: async () => ({ text: '' }),
    notifyBotChat: () => {},
  })
  await runtime.messageBot(A, { text: '记住这个', source: { kind: 'channel', fromOwner: false } })
  assert.equal(runtime.takeBotTurnTrust('conv'), 'external')
  // 取走之后，下一轮由应用里的用户发起：不沾上一轮的外部标记。
  assert.equal(runtime.takeBotTurnTrust('conv'), 'user')
  assert.equal(runtime.takeBotTurnTrust('conv', true), 'agent')
  await runtime.messageBot(A, { text: '记住这个', source: { kind: 'chat' } })
  assert.equal(runtime.takeBotTurnTrust('conv', true), 'user')
  // 正在说话时有人插话：这一轮余下的 bot_remember 按最不可信的算。
  active = true
  value.botTurnTrust = 'user'
  await runtime.messageBot(A, { text: '插一句', source: { kind: 'bot', botId: B } })
  assert.equal(value.botTurnTrust, 'external')
  await runtime.messageBot(A, { text: '再插一句', source: { kind: 'chat' } })
  assert.equal(value.botTurnTrust, 'external')
})

test('the conversation can remember and forget by id, and only writes this bot’s memory', async () => {
  await withService(async (service, { memory }) => {
    const runtime = Object.create(AgentRuntimeFacade.prototype)
    runtime.botMemory = service
    const remembered = await runtime.executeBotDecision(
      'conv',
      { botTurnTrust: 'external' },
      botA,
      {
        type: 'remember',
        text: '发版前在 iPhone 上点一遍首页',
        source: 'told',
      },
    )
    const entry = (await service.list(A))[0]
    assert.equal(remembered.id, entry.id)
    assert.equal(memory.getMemory(entry.id).tainted, true)
    // 上下文里看到的是完整 id，前 6 位也行。
    await runtime.executeBotDecision('conv', {}, botA, {
      type: 'forget',
      memory: entry.id.slice(4, 10),
    })
    assert.deepEqual(await service.list(A), [])
    const full = await service.remember(A, '另一条记忆')
    await runtime.executeBotDecision('conv', {}, botA, { type: 'forget', memory: full.id })
    assert.deepEqual(await service.list(A), [])
    await assert.rejects(
      runtime.executeBotDecision('conv', {}, botA, { type: 'forget', memory: 'nope' }),
      /does not match/,
    )
    assert.deepEqual(
      [
        { type: 'remember', text: '以后别 push 到 main', source: 'correction' },
        { type: 'remember', text: '以后别 push 到 main', again: true },
        { type: 'forget', memory: 'abcdef' },
        { type: 'remember', text: '' },
      ].map(normalizeBotAction),
      [
        { type: 'remember', text: '以后别 push 到 main', source: 'correction' },
        { type: 'remember', text: '以后别 push 到 main', source: 'told', again: true },
        { type: 'forget', memory: 'abcdef' },
        null,
      ],
    )
  })
})

test('what the user deleted stays deleted, unless they ask for it again', async () => {
  await withService(async (service) => {
    const runtime = Object.create(AgentRuntimeFacade.prototype)
    runtime.botMemory = service
    const entry = await service.remember(A, '用户的前公司是某某')
    await service.forget(A, { id: entry.id })
    const skipped = await runtime.executeBotDecision('conv', {}, botA, {
      type: 'remember',
      text: '用户的前公司是某某',
      source: 'told',
    })
    assert.deepEqual(skipped, { type: 'remember', id: '', skipped: 'forgotten' })
    assert.deepEqual(await service.list(A), [])
    const asked = await runtime.executeBotDecision('conv', {}, botA, {
      type: 'remember',
      text: '用户的前公司是某某',
      source: 'told',
      again: true,
    })
    assert.ok(asked.id)
    await service.forget(A, { id: asked.id })
    // 设置页亲手加也算又想记住了。
    assert.ok((await service.remember(A, '用户的前公司是某某', 'manual')).id)
  })
})

test('bot_recall returns labelled results without exposing anything but the text', async () => {
  const calls = []
  const tools = createBotConversationTools({
    runtime: {
      recall: async (input) => {
        calls.push(input)
        return {
          type: 'recall',
          count: 1,
          memories: [
            {
              kind: 'fact',
              id: 'mem_abc123',
              type: 'fact',
              title: '发布窗口',
              content: '周三上午',
              validFrom: '2026-10-01T00:00:00.000Z',
              status: 'superseded',
              tainted: true,
            },
          ],
        }
      },
    },
  })
  const tool = tools.find((item) => item.name === 'bot_recall')
  const result = await tool.execute('call-1', {
    query: '发布窗口',
    queries: ['release window'],
    since: '2026-10-01',
    until: 'not a date',
    include_history: true,
    order: 'recent',
    level: 'digests',
  })
  assert.match(result.content[0].text, /\[mem_abc123\] \[fact\] \[2026-10-01\] 发布窗口/)
  assert.match(result.content[0].text, /replaced by a newer version/)
  assert.match(result.content[0].text, /untrusted source/)
  assert.deepEqual(calls[0], {
    query: '发布窗口',
    queries: ['release window'],
    since: '2026-10-01T00:00:00.000Z',
    until: null,
    includeHistory: true,
    order: 'recent',
    level: 'digests',
  })
})

test('sub-agents get the profile and what relates to their task, not everything', async () => {
  await withService(async (service, { memory }) => {
    const bot = { ...botA, instructions: '负责 acme-web 前端', tools: null }
    memory.profileContext = () => '<pisper_memory_profile>用户偏好中文</pisper_memory_profile>'
    await service.remember(A, '修首页前先在 iPhone 上看一遍首页')
    await service.remember(A, '数据库备份在每周日凌晨跑')
    const spawned = []
    const runtime = Object.create(AgentRuntimeFacade.prototype)
    Object.assign(runtime, {
      botMemory: service,
      botForSession: (id) => (id === 'conv' ? bot : null),
      multiAgents: { list: () => [] },
      permissions: { getPending: () => [] },
      providerPreferences: { resolveSubagentModel: async () => ({ model: null }) },
    })
    // 状态快照里不再塞记忆。
    assert.match(await runtime.botStateContext('conv'), /^\[Current state\]/)
    assert.doesNotMatch(await runtime.botStateContext('conv'), /iPhone/)
    const value = {
      baseToolNames: ['read'],
      multiAgentRuntime: {
        spawn: async (input) => {
          spawned.push(input)
          return { id: 'agent-1' }
        },
      },
    }
    await runtime.executeBotDecision('conv', value, bot, {
      type: 'spawn',
      title: '修首页',
      task: '修好首页，修之前先看一遍',
    })
    assert.match(spawned[0].instructions, /\[小橙's instructions\]\n负责 acme-web 前端/)
    assert.match(spawned[0].instructions, /<pisper_memory_profile>用户偏好中文/)
    assert.match(spawned[0].instructions, /iPhone/)
    assert.doesNotMatch(spawned[0].instructions, /数据库备份/)
  })
})

test('an environment without memory does not take the conversation down', async () => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-bot-memory-off-'))
  try {
    const service = new BotMemoryService({ dataDir, memory: { db: null } })
    await assert.rejects(service.list(A), { code: 'unavailable', statusCode: 503 })
    await assert.rejects(service.remember(A, '记一条'), {
      code: 'unavailable',
      params: { subject: 'memory' },
    })
    assert.deepEqual(await service.turnContext(botA, { message: '你好啊朋友' }), {
      text: '',
      memories: [],
    })
    assert.equal(await service.agentContext(botA, { task: '干活' }), '')
    await service.removeBot(A)
    const failing = new BotMemoryService({
      dataDir,
      memory: { db: {} },
      waitForMemory: async () => {
        throw new Error('初始化失败')
      },
    })
    await assert.rejects(failing.list(A), { code: 'unavailable', params: { subject: 'memory' } })
  } finally {
    await rm(dataDir, { recursive: true, force: true })
  }
})

test('memory waits for deferred initialization before it is used', async () => {
  await withService(async (_service, { dataDir, memory }) => {
    let release = () => {}
    const ready = new Promise((resolve) => {
      release = resolve
    })
    const service = new BotMemoryService({ dataDir, memory, waitForMemory: () => ready })
    let settled = false
    const listing = service.list(A).then((entries) => {
      settled = true
      return entries
    })
    await new Promise((resolve) => setImmediate(resolve))
    assert.equal(settled, false)
    release()
    assert.deepEqual(await listing, [])
  })
})

test('readGlobalMemory defaults on, is kept across edits, and must be a boolean', () => {
  const base = { name: '小橙', workspace: { cwd: '/tmp/acme' } }
  assert.equal(normalizeBotInput(base).readGlobalMemory, true)
  const off = normalizeBotInput({ ...base, readGlobalMemory: false })
  assert.equal(off.readGlobalMemory, false)
  assert.equal(normalizeBotInput({ instructions: '改一下' }, off).readGlobalMemory, false)
  assert.equal(normalizeBotInput({ readGlobalMemory: true }, off).readGlobalMemory, true)
  assert.throws(() => normalizeBotInput({ ...base, readGlobalMemory: 'no' }), {
    code: 'invalid',
    params: { subject: 'memory', detail: 'readGlobalMemory must be true or false' },
  })
})
