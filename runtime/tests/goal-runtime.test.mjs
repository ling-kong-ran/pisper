import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { isGoalContinuationMessage } from '../services/goal-service.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { createSettleBarrierExtension } from '../runtime/settle-barrier.mjs'

test('goal mode queues hidden continuation turns until the goal is completed', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-goal-runtime-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  runtime.archiveAttachments = async () => {}
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  const listeners = new Set()
  const queued = []
  let turns = 0
  const beforeSettle = []
  createSettleBarrierExtension(runtime.settleBarrier)({
    on: (type, handler) => type === 'agent_before_settle' && beforeSettle.push(handler),
  })
  const emit = (event) => {
    for (const listener of listeners) listener(event)
  }
  const session = {
    sessionId: 'goal-session',
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages: [],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => ['get_goal', 'update_goal'],
    setActiveToolsByName: () => {},
    setSessionName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async followUp(text) {
      queued.push(text)
    },
    async prompt(text) {
      session.isStreaming = true
      const processTurn = async (promptText) => {
        session.messages.push({ role: 'user', content: promptText, timestamp: Date.now() })
        emit({ type: 'turn_start' })
        turns += 1
        if (isGoalContinuationMessage(promptText)) await runtime.goals.complete('goal-session')
        const assistant = {
          role: 'assistant',
          content: [{ type: 'text', text: `turn ${turns}` }],
          usage: { totalTokens: 10 },
          timestamp: Date.now(),
        }
        session.messages.push(assistant)
        emit({
          type: 'message_update',
          assistantMessageEvent: { type: 'text_delta', delta: `turn ${turns}` },
        })
        emit({ type: 'turn_end', message: assistant })
        emit({ type: 'agent_end', messages: [assistant] })
      }
      // 与 Pi 一致：每次运行收尾先跑 agent_before_settle 处理器，再看队列决定是否继续。
      const settle = async () => {
        for (const handler of beforeSettle)
          await handler({}, { sessionManager: { getSessionId: () => session.sessionId } })
      }
      await processTurn(text)
      await settle()
      while (queued.length) {
        await processTurn(queued.shift())
        await settle()
      }
      session.isStreaming = false
    },
    async abort() {},
    dispose() {},
  }
  const value = { session, cwd: directory, name: 'Goal test', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Implement the focused Goal.',
    goalMode: true,
    send: (event, data) => events.push({ event, data }),
  })

  assert.equal(turns, 2)
  assert.equal(runtime.getSessionGoal(session.sessionId).status, 'complete')
  assert.equal(events.find((item) => item.event === 'meta').data.goal.status, 'active')
  assert.equal(events.find((item) => item.event === 'done').data.goal.status, 'complete')
  const messages = await runtime.getSessionMessages(session.sessionId)
  assert.deepEqual(
    messages.map((message) => message.role),
    ['user', 'agent', 'agent'],
  )
  assert.deepEqual(
    messages.map((message) => message.text),
    ['Implement the focused Goal.', 'turn 1', 'turn 2'],
  )
})

test('an active Goal keeps its objective when the user sends another Goal turn', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-goal-active-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  runtime.archiveAttachments = async () => {}
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  const original = await runtime.goals.start('active-goal-session', {
    objective: 'Original objective.',
  })
  const session = {
    sessionId: 'active-goal-session',
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages: [],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => ['get_goal', 'update_goal'],
    setActiveToolsByName: () => {},
    setSessionName: () => {},
    subscribe: () => () => {},
    async prompt() {},
  }
  const value = { session, cwd: directory, name: 'Active Goal test', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Add more evidence.',
    goalMode: true,
    send: () => {},
  })
  const current = runtime.getSessionGoal(session.sessionId)
  assert.equal(current.id, original.id)
  assert.equal(current.objective, original.objective)
})

test('goal mode resumes a paused Goal without replacing its objective', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-goal-resume-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  runtime.archiveAttachments = async () => {}
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  const original = await runtime.goals.start('paused-goal-session', {
    objective: 'Finish the original objective.',
  })
  await runtime.goals.pause('paused-goal-session')
  const session = {
    sessionId: 'paused-goal-session',
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages: [{ role: 'user', content: 'Earlier context', timestamp: Date.now() }],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => ['get_goal', 'update_goal'],
    setActiveToolsByName: () => {},
    setSessionName: () => {},
    subscribe: () => () => {},
    async prompt(text) {
      session.messages.push({ role: 'user', content: text, timestamp: Date.now() })
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Continuing the existing Goal.' }],
        timestamp: Date.now(),
      })
    },
  }
  const value = { session, cwd: directory, name: 'Goal resume test', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Continue from where you stopped.',
    goalMode: true,
    send: (event, data) => events.push({ event, data }),
  })

  const resumed = runtime.getSessionGoal(session.sessionId)
  assert.equal(resumed.id, original.id)
  assert.equal(resumed.objective, original.objective)
  assert.equal(resumed.status, 'active')
  assert.equal(events.find((item) => item.event === 'meta').data.goal.id, original.id)
})

test('team mode projects member lifecycle and final summary through the Runtime SSE stream', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-runtime-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  await runtime.teamWorkflows.init()
  runtime.archiveAttachments = async () => {}
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  const listeners = new Set()
  const session = {
    sessionId: 'team-session',
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages: [],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => ['read'],
    setActiveToolsByName: () => {},
    setSessionName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async followUp() {},
    async prompt(text) {
      session.messages.push({ role: 'user', content: text, timestamp: Date.now() })
      for (const listener of listeners) listener({ type: 'turn_start' })
      const first = await runtime.teamWorkflows.registerTask('team-session', {
        taskName: 'inspect',
        role: 'investigator',
        files: ['src/'],
        message: 'Inspect the implementation.',
      })
      await runtime.teamWorkflows.bindAgent('team-session', first.id, {
        id: 'agent-1',
        status: 'running',
      })
      runtime.emitAgentUpdate('team-session', { id: 'agent-1', status: 'running' })
      await runtime.teamWorkflows.updateAgent('team-session', {
        id: 'agent-1',
        status: 'completed',
        output: 'Inspection evidence collected.',
      })
      runtime.emitAgentUpdate('team-session', {
        id: 'agent-1',
        status: 'completed',
        output: 'Inspection evidence collected.',
      })
      const second = await runtime.teamWorkflows.registerTask('team-session', {
        taskName: 'verify',
        role: 'tester',
        files: ['runtime/tests/'],
        dependsOn: ['inspect'],
        message: 'Verify the evidence.',
      })
      await runtime.teamWorkflows.bindAgent('team-session', second.id, {
        id: 'agent-2',
        status: 'completed',
        output: 'Verification passed.',
      })
      runtime.emitAgentUpdate('team-session', {
        id: 'agent-2',
        status: 'completed',
        output: 'Verification passed.',
      })
      const assistant = {
        role: 'assistant',
        content: [{ type: 'text', text: 'Team evidence is complete.' }],
        usage: { totalTokens: 20 },
        timestamp: Date.now(),
      }
      session.messages.push(assistant)
      for (const listener of listeners) {
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'text_delta', delta: assistant.content[0].text },
        })
        listener({ type: 'turn_end', message: assistant })
        listener({ type: 'agent_end', messages: [assistant] })
      }
    },
    async abort() {},
    dispose() {},
  }
  const value = {
    session,
    cwd: directory,
    name: 'Team test',
    baseToolNames: ['read'],
    enabledTools: ['read'],
  }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Coordinate the implementation and verification.',
    teamMode: true,
    send: (event, data) => events.push({ event, data }),
  })

  const meta = events.find((item) => item.event === 'meta')
  const updates = events.filter((item) => item.event === 'agent_update')
  const done = events.find((item) => item.event === 'done')
  assert.equal(meta.data.team.status, 'active')
  assert.ok(updates.some((item) => item.data.team.completedTaskCount === 1))
  assert.equal(done.data.team.completedTaskCount, 2)
  assert.equal(done.data.team.summary.text, 'Team evidence is complete.')
  assert.equal(runtime.getTeamProjection(session.sessionId).taskCount, 2)
  assert.equal(runtime.teamWorkflows.canComplete(session.sessionId).ok, true)
})

test('team goal completion rolls back when the Team workflow cannot be marked complete', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-goal-team-rollback-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  await runtime.teamWorkflows.init()
  const goal = await runtime.goals.start('rollback-session', {
    objective: 'Coordinate and complete a team goal.',
    mode: 'team',
  })
  await runtime.teamWorkflows.ensure('rollback-session', {
    goalId: goal.id,
    objective: goal.objective,
  })
  const task = await runtime.teamWorkflows.registerTask('rollback-session', {
    taskName: 'done',
    files: ['result.txt'],
    message: 'Produce evidence.',
  })
  await runtime.teamWorkflows.bindAgent('rollback-session', task.id, {
    id: 'agent-1',
    status: 'completed',
    output: 'Verified evidence.',
  })
  runtime.multiAgents.hasActive = () => false
  runtime.multiAgents.list = () => []
  runtime.teamWorkflows.markComplete = async () => {
    throw new Error('team write failed')
  }

  const completeGoal = async () => {
    const currentGoal = runtime.goals.get('rollback-session')
    if (currentGoal?.mode === 'team') {
      if (runtime.multiAgents.hasActive('rollback-session'))
        throw new Error('Team 仍有成员在执行，请先等待所有成员完成并完成最终验收。')
      await runtime.teamWorkflows.syncAgents(
        'rollback-session',
        runtime.multiAgents.list('rollback-session'),
      )
      const completion = runtime.teamWorkflows.canComplete('rollback-session')
      if (!completion.ok) throw new Error(completion.reason)
    }
    const completed = await runtime.goals.complete('rollback-session')
    try {
      if (currentGoal?.mode === 'team') await runtime.teamWorkflows.markComplete('rollback-session')
    } catch (error) {
      await runtime.goals.reopen('rollback-session', { goalId: currentGoal?.id })
      throw error
    }
    return completed
  }

  await assert.rejects(completeGoal(), /team write failed/)
  assert.equal(runtime.goals.get('rollback-session').status, 'active')
  assert.equal(runtime.teamWorkflows.get('rollback-session').status, 'active')
})

test('a plan-mode message pauses the active team goal and clears the team snapshot', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-plan-clears-team-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  await runtime.teamWorkflows.init()
  runtime.archiveAttachments = async () => {}
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  const goal = await runtime.goals.start('plan-session', {
    objective: 'Drive the team.',
    mode: 'team',
  })
  await runtime.teamWorkflows.ensure('plan-session', {
    goalId: goal.id,
    objective: goal.objective,
  })

  const listeners = new Set()
  const followUps = []
  const session = {
    sessionId: 'plan-session',
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages: [],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    setSessionName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async followUp(text) {
      followUps.push(text)
    },
    async prompt(text) {
      session.messages.push({ role: 'user', content: text, timestamp: Date.now() })
      for (const listener of listeners) listener({ type: 'turn_start' })
      const assistant = {
        role: 'assistant',
        content: [{ type: 'text', text: 'plan answer' }],
        usage: { totalTokens: 5 },
        timestamp: Date.now(),
      }
      session.messages.push(assistant)
      for (const listener of listeners) {
        listener({ type: 'turn_end', message: assistant })
        listener({ type: 'agent_end', messages: [assistant] })
      }
    },
    async abort() {},
    dispose() {},
  }
  const value = { session, cwd: directory, name: 'Plan test', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Just answer this one question.',
    send: (event, data) => events.push({ event, data }),
  })

  // Plan 消息显式离开目标驱动：目标与团队暂停，不再排队隐藏延续。
  assert.equal(runtime.goals.get('plan-session').status, 'paused')
  assert.equal(runtime.teamWorkflows.get('plan-session').status, 'paused')
  assert.deepEqual(followUps, [])
  // meta/done 都带 team: null 清除信号，前端不再残留团队面板。
  assert.equal(events.find((item) => item.event === 'meta').data.team, null)
  assert.equal(events.find((item) => item.event === 'done').data.team, null)
})

test('goal continuation waits for Pi retries and skips terminal assistant errors', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-goal-retry-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  runtime.archiveAttachments = async () => {}
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  await runtime.goals.start('retry-goal-session', { objective: 'Keep working safely.' })

  const listeners = new Set()
  const queued = []
  const session = {
    sessionId: 'retry-goal-session',
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages: [{ role: 'user', content: 'Earlier context', timestamp: Date.now() }],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => ['get_goal', 'update_goal'],
    setActiveToolsByName: () => {},
    setSessionName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async followUp(text) {
      queued.push(text)
    },
    async prompt(text) {
      session.messages.push({ role: 'user', content: text, timestamp: Date.now() })
      const retrying = {
        role: 'assistant',
        content: [{ type: 'text', text: 'Temporary provider failure.' }],
        stopReason: 'stop',
        timestamp: Date.now(),
      }
      for (const listener of listeners)
        listener({ type: 'agent_end', messages: [retrying], willRetry: true })
      const failed = {
        role: 'assistant',
        content: [{ type: 'text', text: 'The retry did not complete.' }],
        stopReason: 'error',
        timestamp: Date.now(),
      }
      session.messages.push(failed)
      for (const listener of listeners)
        listener({ type: 'agent_end', messages: [failed], willRetry: false })
    },
  }
  const value = { session, cwd: directory, name: 'Goal retry test', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Continue the Goal.',
    // 真实客户端在 Goal 模式下发送消息；不带标记的普通（Plan）消息会暂停目标。
    goalMode: true,
    send: () => {},
  })

  assert.deepEqual(queued, [])
  assert.equal(runtime.getSessionGoal(session.sessionId).status, 'active')
})

// 真实 Pi：预算在用量结算（异步落盘）后才判定耗尽，预算总结要赶在 Pi 收尾前入队，
// 同一次运行里就把总结发出去，而不是停在队列里等用户下次发消息。
test('a real runtime sends the budget summary in the same run once the goal budget is spent', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-goal-settle-'))
  const requests = []
  const server = createServer(async (request, reply) => {
    if (request.method === 'GET' && request.url === '/v1/models') {
      reply.writeHead(200, { 'content-type': 'application/json' })
      reply.end(JSON.stringify({ data: [{ id: 'fixture' }] }))
      return
    }
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    requests.push(JSON.parse(Buffer.concat(chunks).toString('utf8')))
    const chunk = (delta, finish_reason = null) => ({
      id: 'test',
      object: 'chat.completion.chunk',
      created: 0,
      model: 'fixture',
      choices: [{ index: 0, delta, finish_reason }],
    })
    reply.writeHead(200, { 'content-type': 'text/event-stream' })
    reply.write(
      `data: ${JSON.stringify(chunk({ role: 'assistant', content: `answer ${requests.length}` }))}\n\n`,
    )
    reply.write(
      `data: ${JSON.stringify({ ...chunk({}, 'stop'), usage: { prompt_tokens: 300, completion_tokens: 20, total_tokens: 320 } })}\n\n`,
    )
    reply.end('data: [DONE]\n\n')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  let runtime
  t.after(async () => {
    await runtime?.dispose()
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
    await rm(directory, { recursive: true, force: true })
  })
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        'test-goal': {
          api: 'openai-completions',
          baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
          models: [
            {
              id: 'fixture',
              name: 'Fixture',
              reasoning: false,
              input: ['text'],
              contextWindow: 200000,
              maxTokens: 8192,
            },
          ],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({ 'test-goal': { type: 'api_key', key: 'test-only-goal-key' } }),
  )
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.memorySummarizer = { summarize: async (entries) => entries.map(() => '') }
  await runtime.init()
  runtime.memory.setSemanticSummarizer(null)
  const created = await runtime.createSession('预算目标', directory)
  await runtime.setSessionModel(created.id, 'test-goal', 'fixture')
  await runtime.streamPrompt({
    sessionId: created.id,
    message: 'Finish the small goal.',
    goalMode: true,
    goalTokenBudget: 100,
    send: () => {},
  })

  assert.equal(runtime.getSessionGoal(created.id).status, 'budget_limited')
  assert.equal(requests.length, 2)
  const lastUser = requests[1].messages.filter((message) => message.role === 'user').at(-1)
  assert.match(JSON.stringify(lastUser), /reached its token budget/)
  const live = await runtime.getExistingSession(created.id)
  assert.equal(live.session.agent.hasQueuedMessages(), false)
})
