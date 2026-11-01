// 每轮注入上下文的去重：完整的目标/团队提示与记忆只在当前上下文里缺失时才注入。
import assert from 'node:assert/strict'
import test from 'node:test'
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
  injectedMemoryIds,
} from '../runtime/injected-context.mjs'

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
