// Bot 自己写的技能（C4）：做完一件活复盘，会反复做的写成标准 SKILL.md，按 Bot 隔离；
// 它派出的子 agent 带上这个技能目录；设置页“它学会的”能看、能删，删掉的不再被带上。
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { botReviewInstruction, parseBotSkillReview } from '../services/bots/bot-conversation.mjs'
import {
  BotSkillService,
  MAX_BOT_SKILLS,
  botSkillsContext,
} from '../services/bots/bot-skill-service.mjs'
import { MultiAgentService } from '../services/multi-agent-service.mjs'

const BOT = 'bot_aaaaaaaaaaaa'

async function withDir(run) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-bot-skills-'))
  try {
    await run(dataDir)
  } finally {
    await rm(dataDir, { recursive: true, force: true })
  }
}

const releaseCheck = {
  name: 'release-check',
  description: '发版前的检查：构建、跑测试、在 iPhone 上点一遍首页',
  body: '1. npm run build\n2. npm test\n3. iPhone 上点一遍首页',
}

test('skills are standard SKILL.md files, kept per bot, updated by name and removable', async () => {
  await withDir(async (dataDir) => {
    const skills = new BotSkillService({ dataDir })
    const created = await skills.write(BOT, {
      ...releaseCheck,
      description: '发版前检查: "构建" 与测试',
    })
    assert.equal(created.created, true)
    const file = join(dataDir, 'bots', BOT, 'skills', 'release-check', 'SKILL.md')
    const content = await readFile(file, 'utf8')
    assert.match(content, /^---\nname: release-check\ndescription: ".*"\n---\n\n1\. npm run build/)
    assert.equal((await skills.get(BOT, 'release-check')).description, '发版前检查: "构建" 与测试')
    const updated = await skills.write(BOT, { ...releaseCheck, body: '1. npm run check' })
    assert.equal(updated.created, false)
    assert.equal((await skills.get(BOT, 'release-check')).body, '1. npm run check')
    assert.deepEqual(await skills.list('bot_bbbbbbbbbbbb'), [])
    assert.deepEqual(
      (await skills.list(BOT)).map((skill) => skill.name),
      ['release-check'],
    )
    await assert.rejects(skills.write(BOT, { ...releaseCheck, name: '../x' }), {
      code: 'bot_skill_invalid',
    })
    await assert.rejects(skills.get(BOT, '../../memory'), { code: 'bot_skill_not_found' })
    await assert.rejects(skills.list('../etc'), { code: 'bot_not_found' })
    assert.equal(await skills.remove(BOT, 'release-check'), true)
    assert.equal(await skills.remove(BOT, 'release-check'), false)
    assert.equal(existsSync(file), false)
  })
})

test('each bot has a bounded number of skills', async () => {
  await withDir(async (dataDir) => {
    const skills = new BotSkillService({ dataDir })
    for (let index = 0; index < MAX_BOT_SKILLS; index += 1)
      await skills.write(BOT, { ...releaseCheck, name: `skill-${index}` })
    await assert.rejects(skills.write(BOT, { ...releaseCheck, name: 'one-more' }), {
      code: 'bot_skill_limit',
    })
    // 同名更新不算新增。
    await skills.write(BOT, { ...releaseCheck, name: 'skill-0', body: '新的做法' })
  })
})

test('the review is parsed strictly; anything malformed means no skill', () => {
  assert.deepEqual(
    parseBotSkillReview(`好的\n${JSON.stringify({ skill: releaseCheck })}`),
    releaseCheck,
  )
  assert.equal(parseBotSkillReview('{"skill": null}'), null)
  assert.equal(parseBotSkillReview('not json'), null)
  assert.equal(
    parseBotSkillReview(JSON.stringify({ skill: { ...releaseCheck, name: 'Bad Name' } })),
    null,
  )
  assert.equal(parseBotSkillReview(JSON.stringify({ skill: { ...releaseCheck, body: '' } })), null)
  const instruction = botReviewInstruction({
    title: '发版前检查',
    task: '构建并测试',
    output: '都通过了',
    skills: [releaseCheck],
    corrections: ['以后别跳过 iPhone 检查'],
  })
  assert.match(instruction, /Existing skills: release-check \(/)
  assert.match(instruction, /User corrections to respect:\n- 以后别跳过 iPhone 检查/)
  assert.match(botSkillsContext([releaseCheck]), /^\[你会的技能\].*\n- release-check：发版前的检查/)
  assert.equal(botSkillsContext([]), '')
})

function reviewRuntime(dataDir, reply) {
  const prompts = []
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  const bot = {
    id: BOT,
    name: '小橙',
    threadSessionId: 'conv',
    model: { tier: 'standard' },
    instructions: '',
    tools: null,
  }
  Object.assign(runtime, {
    bots: { get: (id) => (id === BOT ? bot : null) },
    botSkills: new BotSkillService({ dataDir }),
    botMemory: {
      list: async () => [
        { id: 'mem_aaaaaa', text: '以后别跳过 iPhone 检查', source: 'correction' },
      ],
    },
    botForSession: (id) => (id === 'conv' ? bot : null),
    sessions: new Map([['conv', { session: { model: { provider: 'luna', id: 'm' } } }]]),
    providerPreferences: { resolveSubagentModel: async () => ({ model: null }) },
    modelRuntime: {
      streamSimple: (_model, context) => {
        prompts.push(context.messages[0].content[0].text)
        return {
          async *[Symbol.asyncIterator]() {
            yield { type: 'text_delta', delta: reply }
          },
          result: async () => ({ stopReason: 'stop', usage: null }),
        }
      },
    },
    recordUsage: async () => {},
  })
  return { runtime, prompts }
}

test('a finished job is reviewed and repeatable work becomes the bot’s own skill', async () => {
  await withDir(async (dataDir) => {
    const { runtime, prompts } = reviewRuntime(dataDir, JSON.stringify({ skill: releaseCheck }))
    const job = {
      id: 'agent-1',
      parentSessionId: 'conv',
      status: 'completed',
      title: '发版前检查',
      taskName: 'release_check',
      message: '构建、测试、iPhone 上点首页',
      fullOutput: '都通过了',
    }
    await runtime.botAgentFinished(job)
    assert.match(prompts[0], /Job: 发版前检查/)
    assert.match(prompts[0], /以后别跳过 iPhone 检查/)
    assert.deepEqual(
      (await runtime.listBotSkills(BOT)).map((skill) => skill.name),
      ['release-check'],
    )
    // 失败、被停的活不复盘；别的会话（不是 Bot 的对话）也不复盘。
    assert.equal(runtime.botAgentFinished({ ...job, status: 'failed' }), null)
    assert.equal(runtime.botAgentFinished({ ...job, parentSessionId: 'other' }), null)
    // 复盘说不用写时不写。
    const quiet = reviewRuntime(dataDir, '{"skill": null}')
    await quiet.runtime.botAgentFinished(job)
    assert.equal((await runtime.listBotSkills(BOT)).length, 1)
  })
})

test('sub-agents the bot hands work to carry its skills; a deleted skill is no longer carried', async () => {
  await withDir(async (dataDir) => {
    const skills = new BotSkillService({ dataDir })
    await skills.write(BOT, releaseCheck)
    const spawned = []
    const runtime = Object.create(AgentRuntimeFacade.prototype)
    const bot = { id: BOT, name: '小橙', instructions: '', tools: null }
    Object.assign(runtime, {
      bots: { get: (id) => (id === BOT ? bot : null) },
      botSkills: skills,
      botMemory: { list: async () => [] },
      botForSession: () => bot,
      multiAgents: { list: () => [] },
      permissions: { getPending: () => [] },
      providerPreferences: { resolveSubagentModel: async () => ({ model: null }) },
    })
    assert.match(await runtime.botStateContext('conv'), /\[你会的技能\][^\n]*\n- release-check：/)
    await runtime.executeBotDecision(
      'conv',
      {
        baseToolNames: ['read'],
        multiAgentRuntime: {
          spawn: async (input) => {
            spawned.push(input)
            return { id: 'agent-1' }
          },
        },
      },
      bot,
      { type: 'spawn', title: '发版前检查', task: '按技能 release-check 做' },
    )
    assert.deepEqual(spawned[0].skillPaths, [join(dataDir, 'bots', BOT, 'skills')])

    // 多 Agent 服务把这个目录交给技能加载器。
    const loaderCalls = []
    const service = new MultiAgentService({
      path: join(dataDir, 'agents.json'),
      agentDir: dataDir,
      getModelRuntime: () => null,
      createResourceLoader: async (options) => {
        loaderCalls.push(options)
        throw new Error('stop here')
      },
    })
    await service.spawn({
      parentSessionId: 'conv',
      cwd: dataDir,
      model: { provider: 'luna', id: 'm' },
      taskName: 'release_check',
      message: '按技能 release-check 做',
      skillPaths: spawned[0].skillPaths,
    })
    // 等这条子 agent 失败落盘（加载器在这里抛错），再收拾临时目录。
    for (let index = 0; index < 200; index += 1) {
      if (service.list('conv')[0]?.status === 'failed') break
      await new Promise((resolve) => setTimeout(resolve, 5))
    }
    await service.save()
    assert.deepEqual(loaderCalls[0].skillPaths, spawned[0].skillPaths)

    assert.equal(await runtime.deleteBotSkill(BOT, 'release-check'), true)
    assert.equal(existsSync(join(dataDir, 'bots', BOT, 'skills', 'release-check')), false)
  })
})
