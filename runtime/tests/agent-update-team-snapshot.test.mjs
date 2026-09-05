import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

function eventRuntime({ attachTeam, status = 'paused', withLive = true } = {}) {
  const runtime = Object.create(AgentRuntimeService.prototype)
  const goal = { id: 'goal-1', mode: 'team', status, tokensUsed: 12 }
  const team = {
    id: 'team-1',
    status,
    tasks: [{ id: 'task-1', taskName: 'inspect', status: 'completed' }],
    summary: { text: 'Existing evidence' },
  }
  const agent = {
    id: 'agent-250',
    canonicalName: '/root/speech-terms-ui_250',
    taskName: 'speech-terms-ui',
    status: 'running',
  }
  const live = { attachTeam, goal, team: null, activityFeed: [] }
  runtime.liveSessions = new Map(withLive ? [['session-1', live]] : [])
  runtime.goals = { get: () => goal }
  runtime.teamWorkflows = { get: () => team }
  runtime.multiAgents = { summaries: () => [agent] }
  runtime.streamProjection = { invalidate() {} }
  runtime.agentEmitters = new Map()
  runtime.goalEmitters = new Map()
  const events = []
  const send = (event, data) => events.push({ event, data })
  return { runtime, goal, team, agent, live, events, send }
}

test('ordinary turn agent and goal events keep an explicit team clear signal', () => {
  const { runtime, goal, team, agent, live, events, send } = eventRuntime({ attachTeam: false })
  const originalTeam = structuredClone(team)
  runtime.emitAgentUpdate('session-1', agent, send)
  runtime.emitGoalUpdate('session-1', goal, send)
  assert.deepEqual(
    events.map(({ event }) => event),
    ['agent_update', 'goal_update'],
  )
  for (const { data } of events) assert.equal(data.team, null)
  assert.equal(live.team, null)
  assert.equal(live.goal, goal)
  assert.equal(events[0].data.agent.canonicalName, '/root/speech-terms-ui_250')
  assert.equal(events[0].data.agent.taskName, 'speech-terms-ui')
  assert.deepEqual(team, originalTeam)
})

test('team turns retain paused and completed history while goal transitions use the new goal', () => {
  const { runtime, goal, team, agent, live, events, send } = eventRuntime({ attachTeam: true })
  runtime.emitAgentUpdate('session-1', agent, send)
  assert.equal(live.team.status, 'paused')
  team.status = 'complete'
  const completedGoal = { ...goal, status: 'complete', tokensUsed: 42 }
  runtime.emitGoalUpdate('session-1', completedGoal, send)
  assert.equal(live.team.status, 'complete')
  assert.equal(live.team.tokenUsed, 42)
  assert.equal(live.team.summary.text, 'Existing evidence')
  assert.equal(live.goal, completedGoal)
  runtime.emitGoalUpdate('session-1', { ...goal, mode: 'goal', status: 'active' }, send)
  assert.equal(live.team, null)
  runtime.emitGoalUpdate('session-1', null, send)
  assert.equal(live.team, null)
  assert.equal(live.goal, null)
  assert.equal(events.at(-1).data.team, null)
})

test('updates without a live turn attach active goals but not paused historical teams', () => {
  for (const status of ['active', 'paused', 'complete']) {
    const { runtime, goal, agent, events, send } = eventRuntime({ status, withLive: false })
    runtime.emitAgentUpdate('session-1', agent, send)
    runtime.emitGoalUpdate('session-1', goal, send)
    for (const { data } of events) assert.equal(Boolean(data.team), status === 'active')
  }
})

test('a real ordinary stream does not restore a paused team during or after agent updates', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-event-policy-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  await runtime.teamWorkflows.init()
  runtime.archiveAttachments = async () => {}
  runtime.captureConversationMemory = async () => []
  const id = 'ordinary-session'
  const goal = await runtime.goals.start(id, { objective: 'Earlier team', mode: 'team' })
  await runtime.teamWorkflows.ensure(id, { goalId: goal.id, objective: goal.objective })
  await runtime.teamWorkflows.setSummary(id, 'Keep historical evidence')
  const agent = {
    id: 'agent-250',
    canonicalName: '/root/speech-terms-ui_250',
    taskName: 'speech-terms-ui',
    status: 'running',
  }
  runtime.multiAgents.summaries = () => [agent]
  const session = {
    sessionId: id,
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages: [],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName() {},
    setSessionName() {},
    subscribe: () => () => {},
    async prompt(text) {
      runtime.emitAgentUpdate(id, agent)
      runtime.emitGoalUpdate(id, runtime.goals.get(id))
      session.messages.push({ role: 'user', content: text, timestamp: Date.now() })
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Ordinary answer' }],
        timestamp: Date.now(),
      })
    },
  }
  const value = { session, cwd: directory, name: 'Ordinary turn', baseToolNames: [] }
  runtime.sessions.set(id, value)
  runtime.getOrCreateSession = async () => value
  const events = []
  await runtime.streamPrompt({
    sessionId: id,
    message: 'Answer without the earlier team.',
    send: (event, data) => events.push({ event, data }),
  })
  for (const event of ['meta', 'agent_update', 'goal_update', 'done']) {
    const update = events.find((item) => item.event === event)
    assert.ok(update, `Missing ${event}: ${JSON.stringify(events)}`)
    assert.equal(update.data.team, null, event)
  }
  runtime.emitAgentUpdate(id, agent, (event, data) => events.push({ event, data }))
  assert.equal(events.at(-1).data.team, null)
  assert.equal(runtime.liveSessions.get(id).team, null)
  assert.equal(runtime.teamWorkflows.get(id).summary.text, 'Keep historical evidence')
  assert.equal(runtime.goals.get(id).status, 'paused')
})
