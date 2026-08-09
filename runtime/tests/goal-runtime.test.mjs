import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { isGoalContinuationMessage } from '../services/goal-service.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

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
      await processTurn(text)
      while (queued.length) await processTurn(queued.shift())
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
    send: () => {},
  })

  assert.deepEqual(queued, [])
  assert.equal(runtime.getSessionGoal(session.sessionId).status, 'active')
})
