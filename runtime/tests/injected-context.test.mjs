// 每轮注入上下文的去重：完整的目标/团队提示与记忆只在当前上下文里缺失时才注入。
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  GOAL_CONTINUATION_MARKER,
  goalContinuationPrompt,
  hasFullGoalContinuation,
} from '../services/goal-service.mjs'
import {
  hasFullTeamExecutionPrompt,
  TEAM_EXECUTION_MARKER,
  teamExecutionPrompt,
} from '../services/team-workflow.mjs'
import {
  briefContinuation,
  contextUserTexts,
  hasInjectedProfile,
  injectedMemoryIds,
} from '../runtime/injected-context.mjs'
import { ATTACHMENT_MARKER } from '../runtime/stream-projection.mjs'

const goal = {
  objective: 'Ship the login refactor with passing tests.',
  mode: 'team',
  tokensUsed: 10,
  tokenBudget: null,
  teamTokenBudget: null,
  timeUsedSeconds: 3,
}
const user = (text) => ({ role: 'user', content: [{ type: 'text', text }], timestamp: 1 })

test('continuations send the full goal and team rules once, then only a brief status', () => {
  const full = `${goalContinuationPrompt(goal)}\n\n${teamExecutionPrompt(goal, [], { tasks: [] })}`
  assert.equal(hasFullGoalContinuation(full, goal), true)
  assert.equal(hasFullTeamExecutionPrompt(full, goal), true)
  assert.deepEqual(briefContinuation([], goal), { goal: false, team: false })

  // 第一轮的完整提示跟在用户消息的附件上下文之后，同样算作已在上下文里。
  const texts = contextUserTexts([
    user(`做重构\n\n---\nAttachment context (injected by Pisper):\n${full}`),
    { role: 'assistant', content: [{ type: 'text', text: full }] },
  ])
  assert.deepEqual(briefContinuation(texts, goal), { goal: true, team: true })

  const brief = goalContinuationPrompt(goal, { brief: true })
  const briefTeam = teamExecutionPrompt(goal, [{ id: 'a1', status: 'running' }], null, {
    brief: true,
  })
  // 精简版保留内部标记、预算与当前状态，但不再重复目标原文与规则。
  assert.ok(brief.startsWith(GOAL_CONTINUATION_MARKER))
  assert.match(brief, /10\/unlimited tokens used/)
  assert.doesNotMatch(brief, /Ship the login refactor/)
  assert.ok(briefTeam.startsWith(TEAM_EXECUTION_MARKER))
  assert.match(briefTeam, /Current teammate status: a1: running/)
  assert.doesNotMatch(briefTeam, /Team operating rules/)
  assert.ok(brief.length + briefTeam.length < full.length / 3)
  // 精简版不会被误认成完整版本：压缩把完整版本总结掉后，下一轮会重新发完整版。
  assert.deepEqual(briefContinuation([brief, briefTeam], goal), { goal: false, team: false })
  // 目标改了，旧目标的完整提示不算数。
  assert.deepEqual(briefContinuation(texts, { ...goal, objective: 'Something else' }), {
    goal: false,
    team: false,
  })
  // 非 team 目标只看目标延续。
  assert.deepEqual(briefContinuation(texts, { ...goal, mode: 'goal' }), {
    goal: true,
    team: false,
  })
})

test('memory ids already injected into the context are collected for exclusion', () => {
  const memory = [
    '<pisper_memory_context>',
    '<memory id="m-1" type="fact">',
    '</memory>',
    '<memory id="m-2" type="fact">',
    '</memory>',
    '</pisper_memory_context>',
  ].join('\n')
  const texts = contextUserTexts([
    user(`问题\n\n---\nAttachment context (injected by Pisper):\n${memory}`),
    // 用户自己贴的同名标签不算注入。
    user('<memory id="typed-by-user">'),
  ])
  assert.deepEqual(injectedMemoryIds(texts), ['m-1', 'm-2'])
})

test('the injected profile is recognised only in the injected part of a message', () => {
  const profile = '<pisper_memory_profile>\n- 回复用中文\n</pisper_memory_profile>'
  assert.equal(hasInjectedProfile([`问题${ATTACHMENT_MARKER}${profile}`]), true)
  assert.equal(hasInjectedProfile(['用户自己贴的 <pisper_memory_profile>']), false)
  assert.equal(hasInjectedProfile([`问题${ATTACHMENT_MARKER}别的上下文`]), false)
  assert.equal(hasInjectedProfile([]), false)
})

function profileFixture(directory, messages) {
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.captureConversationMemory = async () => []
  const requests = []
  runtime.memory = {
    relevantContext: async () => ({ text: '', memories: [] }),
    resolveSpaceIds: ({ cwd }) => ['global', `project:${cwd}`],
    profileContext: (spaceIds) => {
      requests.push(spaceIds)
      return '<pisper_memory_profile>回复用中文</pisper_memory_profile>'
    },
  }
  const prompts = []
  const session = {
    sessionId: 'memory-profile',
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages,
    agent: { state: { systemPrompt: 'unchanged' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    setSessionName: () => {},
    subscribe: () => () => {},
    async prompt(text) {
      prompts.push(text)
      session.messages.push({ role: 'user', content: text })
    },
    async abort() {},
    dispose() {},
  }
  const value = {
    session,
    cwd: directory,
    name: '记忆',
    baseToolNames: [],
    enabledTools: ['memory_search'],
  }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value
  const send = (message, options = {}) =>
    runtime.streamPrompt({ sessionId: session.sessionId, message, send: () => {}, ...options })
  return { runtime, session, value, prompts, requests, send }
}

test('the profile goes in once, on the first turn, and again only after compaction removes it', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-memory-profile-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const { session, prompts, requests, send } = profileFixture(directory, [])

  await send('第一轮')
  assert.match(
    prompts[0],
    /^第一轮[\s\S]*<pisper_memory_profile>回复用中文<\/pisper_memory_profile>/,
  )
  assert.deepEqual(requests, [['global', `project:${directory}`]])

  await send('第二轮')
  assert.doesNotMatch(prompts[1], /pisper_memory_profile/)
  assert.equal(requests.length, 1)

  // 压缩把早先的消息总结掉了：下一轮重新注入。
  session.messages.splice(0, session.messages.length, {
    role: 'user',
    content: '压缩后的摘要',
  })
  await send('第三轮')
  assert.match(prompts[2], /<pisper_memory_profile>/)
  assert.equal(requests.length, 2)
  // 不碰系统提示：提示缓存前缀保持稳定。
  assert.doesNotMatch(session.agent.state.systemPrompt, /pisper_memory_profile/)
})

test('an isolated session gets no profile from here, and a broken profile does not stop the turn', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-memory-profile-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const isolated = profileFixture(directory, [])
  await isolated.send('Bot 这边的一轮', { isolatedContext: true })
  assert.doesNotMatch(isolated.prompts[0], /pisper_memory_profile/)
  assert.equal(isolated.requests.length, 0)

  const broken = profileFixture(directory, [])
  broken.runtime.memory.profileContext = () => {
    throw new Error('记忆库读不出来')
  }
  await broken.send('照常继续')
  assert.match(broken.prompts[0], /^照常继续/)
  assert.doesNotMatch(broken.prompts[0], /pisper_memory_profile/)
})

test('memory retrieval starts before attachments are prepared and still lands in the prompt', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-memory-early-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.captureConversationMemory = async () => []
  const order = []
  let releaseMemory
  runtime.memory = {
    relevantContext: (query, _cwd, _limit, options) => {
      order.push(['memory', query, options.excludeIds])
      return new Promise((resolve) => {
        releaseMemory = () => resolve({ text: '<pisper_memory_context>m</pisper_memory_context>' })
      })
    },
  }
  runtime.preparePromptAttachments = async () => {
    order.push(['attachments'])
    // 检索还没返回时附件已经在准备：两者并行。
    releaseMemory()
    return { images: [], contexts: [] }
  }
  let prompted = ''
  const session = {
    sessionId: 'memory-early',
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages: [],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    setSessionName: () => {},
    subscribe: () => () => {},
    async prompt(text) {
      prompted = text
    },
    async abort() {},
    dispose() {},
  }
  const value = {
    session,
    cwd: directory,
    name: '记忆',
    baseToolNames: [],
    enabledTools: ['memory_search'],
  }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: '回忆之前的约定',
    send: () => {},
  })

  assert.deepEqual(order, [['memory', '回忆之前的约定', []], ['attachments']])
  assert.match(prompted, /^回忆之前的约定[\s\S]*<pisper_memory_context>m<\/pisper_memory_context>/)
})
