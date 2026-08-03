import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  GOAL_CONTINUATION_MARKER,
  GoalService,
  goalBudgetPrompt,
  goalContinuationPrompt,
  isGoalContinuationMessage,
} from '../services/goal-service.mjs'
import { createGoalTools } from '../tools/app/goal.mjs'

test('goals persist usage, stop at their token budget, and pause after runtime restart', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-goal-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'pisper-goals.json')
  let now = 1_700_000_000_000
  const service = new GoalService({ path, now: () => now })
  await service.init()

  const started = await service.start('session-goal', {
    objective: 'Implement the feature and verify tests.',
    tokenBudget: 100,
  })
  assert.equal(started.status, 'active')
  assert.equal(started.tokensUsed, 0)

  now += 4_000
  const active = await service.account('session-goal', {
    goalId: started.id,
    usage: { totalTokens: 60 },
    elapsedSeconds: 4,
  })
  assert.equal(active.status, 'active')
  assert.equal(active.tokensUsed, 60)
  assert.equal(active.timeUsedSeconds, 4)

  now += 3_000
  const limited = await service.account('session-goal', {
    goalId: started.id,
    usage: { input: 25, output: 20 },
    elapsedSeconds: 3,
  })
  assert.equal(limited.status, 'budget_limited')
  assert.equal(limited.tokensUsed, 105)

  const restarted = new GoalService({ path })
  await restarted.init({ pauseActive: true })
  assert.equal(restarted.get('session-goal').status, 'budget_limited')

  const second = await service.start('session-active', { objective: 'Finish the follow-up.' })
  assert.equal(second.status, 'active')
  const afterReload = new GoalService({ path })
  await afterReload.init({ pauseActive: true })
  assert.equal(afterReload.get('session-active').status, 'paused')
})

test('goal prompts use an internal marker that can be hidden from the transcript', async () => {
  const goal = {
    objective: 'Refactor the runtime and verify the focused tests.',
    tokensUsed: 120,
    tokenBudget: 30_000,
    timeUsedSeconds: 9,
  }
  const continuation = goalContinuationPrompt(goal)
  assert.ok(continuation.startsWith(GOAL_CONTINUATION_MARKER))
  assert.equal(isGoalContinuationMessage(continuation), true)
  assert.match(continuation, /completion audit/i)
  assert.match(goalBudgetPrompt(goal), /token budget/i)
  assert.equal(isGoalContinuationMessage('ordinary user message'), false)
})

test('goal tools only expose completion after the runtime confirms it', async () => {
  let goal = { id: 'goal-1', status: 'active', objective: 'Ship the feature.' }
  const [getGoal, updateGoal] = createGoalTools({
    getGoal: () => goal,
    completeGoal: async () => {
      goal = { ...goal, status: 'complete' }
      return goal
    },
  })
  const current = await getGoal.execute('get-goal', {}, new AbortController().signal)
  assert.match(current.content[0].text, /Ship the feature/)
  const updated = await updateGoal.execute(
    'update-goal',
    { status: 'complete' },
    new AbortController().signal,
  )
  assert.match(updated.content[0].text, /complete/)
  assert.equal(goal.status, 'complete')
})
