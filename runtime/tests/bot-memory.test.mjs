// Bot 自己的记忆：一条条短句，按 Bot 隔离；记住时去重；整理只能保留或改写已有条目；
// 对话的“记住 / 忘掉”决定写进它自己的那份；设置页的增改删走 /api/bots/:id/memory。
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { parseTidy } from '../runtime/bot-decisions.mjs'
import { parseBotDecisions } from '../services/bots/bot-conversation.mjs'
import { BotMemoryService, botMemoryContext } from '../services/bots/bot-memory-service.mjs'

async function withService(run) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-bot-memory-'))
  try {
    await run(new BotMemoryService({ dataDir }), dataDir)
  } finally {
    await rm(dataDir, { recursive: true, force: true })
  }
}

test('each bot keeps its own memory; the same thing is not remembered twice', async () => {
  await withService(async (memory) => {
    const first = await memory.remember('bot_aaaaaaaaaaaa', '以后别 push 到 main。', 'correction')
    const again = await memory.remember('bot_aaaaaaaaaaaa', '以后别push到main', 'told')
    assert.equal(again.id, first.id)
    await memory.remember('bot_bbbbbbbbbbbb', '发版前在 iPhone 上点一遍首页')
    assert.deepEqual(
      (await memory.list('bot_aaaaaaaaaaaa')).map((entry) => [entry.text, entry.source]),
      [['以后别 push 到 main。', 'correction']],
    )
    assert.equal((await memory.list('bot_bbbbbbbbbbbb')).length, 1)
    const edited = await memory.edit('bot_aaaaaaaaaaaa', first.id, '别直接 push 到 main，走 PR')
    assert.equal(edited.text, '别直接 push 到 main，走 PR')
    assert.equal((await memory.forget('bot_aaaaaaaaaaaa', { id: first.id }))?.id, first.id)
    assert.equal(await memory.forget('bot_aaaaaaaaaaaa', { id: first.id }), null)
    await assert.rejects(memory.remember('bot_aaaaaaaaaaaa', '  '), { code: 'bot_memory_empty' })
    await assert.rejects(memory.remember('bot_aaaaaaaaaaaa', 'x'.repeat(501)), {
      code: 'bot_memory_too_long',
    })
    await assert.rejects(memory.list('../etc'), { code: 'bot_not_found' })
    await memory.removeBot('bot_bbbbbbbbbbbb')
    assert.deepEqual(await memory.list('bot_bbbbbbbbbbbb'), [])
  })
})

test('tidying can only keep or reword existing entries, never add new ones', async () => {
  await withService(async (memory) => {
    const bot = 'bot_cccccccccccc'
    const a = await memory.remember(bot, '测试用 vitest')
    const b = await memory.remember(bot, '测试框架是 vitest')
    const c = await memory.remember(bot, '部署走 npm run deploy:preview')
    const kept = parseTidy(
      JSON.stringify({
        kept: [
          { id: a.id, text: '测试框架是 vitest' },
          { id: c.id, text: c.text },
          { id: 'mem_invented123', text: '凭空多出来的' },
          { id: a.id, text: '重复' },
        ],
      }),
      await memory.list(bot),
    )
    assert.deepEqual(
      kept.map((item) => item.id),
      [a.id, c.id],
    )
    const after = await memory.replaceAfterTidy(bot, kept)
    assert.deepEqual(
      after.map((entry) => [entry.id, entry.text]),
      [
        [a.id, '测试框架是 vitest'],
        [c.id, '部署走 npm run deploy:preview'],
      ],
    )
    assert.ok(!after.some((entry) => entry.id === b.id))
    assert.ok((await memory.read(bot)).tidiedAt)
    assert.equal(parseTidy('not json', after), null)
    assert.equal(parseTidy('{"kept":[]}', after), null)
  })
})

test('the conversation sees its memory with short ids, and can remember or forget', () => {
  const context = botMemoryContext([
    {
      id: 'mem_abcdef123456',
      text: '发版前在 iPhone 上点一遍首页',
      updatedAt: '2026-11-04T01:00:00Z',
    },
  ])
  assert.equal(context, '[你记得的事]\n- (abcdef) 发版前在 iPhone 上点一遍首页')
  assert.equal(botMemoryContext([]), '')
  assert.deepEqual(
    parseBotDecisions(
      '{"actions":[{"type":"remember","text":"以后别 push 到 main","source":"correction"},{"type":"forget","memory":"abcdef"},{"type":"remember","text":""}]}',
    ),
    [
      { type: 'remember', text: '以后别 push 到 main', source: 'correction' },
      { type: 'forget', memory: 'abcdef' },
    ],
  )
})

test('remember and forget decisions write only to this bot’s memory', async () => {
  await withService(async (memory) => {
    const runtime = Object.create(AgentRuntimeFacade.prototype)
    runtime.botMemory = memory
    const bot = { id: 'bot_dddddddddddd', name: '小橙', instructions: '' }
    const remembered = await runtime.executeBotDecision('conv', {}, bot, {
      type: 'remember',
      text: '发版前在 iPhone 上点一遍首页',
      source: 'told',
    })
    const entry = (await memory.list(bot.id))[0]
    assert.equal(remembered.id, entry.id)
    await runtime.executeBotDecision('conv', {}, bot, {
      type: 'forget',
      memory: entry.id.slice(4, 10),
    })
    assert.deepEqual(await memory.list(bot.id), [])
    await assert.rejects(
      runtime.executeBotDecision('conv', {}, bot, { type: 'forget', memory: 'nope' }),
      /对不上/,
    )
  })
})

test('the conversation and the sub-agents it hands work to both get the bot’s memory', async () => {
  await withService(async (memory) => {
    const bot = {
      id: 'bot_eeeeeeeeeeee',
      name: '小橙',
      instructions: '负责 acme-web 前端',
      tools: null,
    }
    await memory.remember(bot.id, '发版前在 iPhone 上点一遍首页')
    const spawned = []
    const runtime = Object.create(AgentRuntimeFacade.prototype)
    Object.assign(runtime, {
      botMemory: memory,
      botForSession: (id) => (id === 'conv' ? bot : null),
      multiAgents: { list: () => [] },
      permissions: { getPending: () => [] },
      providerPreferences: { resolveSubagentModel: async () => ({ model: null }) },
    })
    const context = await runtime.botStateContext('conv')
    assert.match(
      context,
      /^\[当前状态\][\s\S]*\[你记得的事\]\n- \([\w-]{6}\) 发版前在 iPhone 上点一遍首页$/,
    )
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
      task: '修好首页',
    })
    assert.match(spawned[0].instructions, /\[小橙 的工作说明\]\n负责 acme-web 前端/)
    assert.match(
      spawned[0].instructions,
      /\[你记得的事\]\n- \([\w-]{6}\) 发版前在 iPhone 上点一遍首页/,
    )
  })
})
