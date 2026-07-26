import assert from 'node:assert/strict'
import { appendFile, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService, multiAgentResultAgent, waitForAgentMailbox } from '../runtime/agent-runtime.mjs'
import { applyTextPatch } from '../../src/lib/api.ts'

test('live session snapshot restores partial assistant output and tool state', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-live-session-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.multiAgents = { summaries: () => [
    { id: 'agent-live', canonicalName: '/root/live_1', status: 'running' },
    { id: 'agent-finished', canonicalName: '/root/finished_1', status: 'completed' },
  ] }
  runtime.sessions.set('session-live', {
    cwd: directory,
    session: {
      isStreaming: true,
      model: { provider: 'openai', id: 'gpt-5.4' },
      messages: [{ role: 'user', content: '继续处理', timestamp: 1 }],
    },
  })
  runtime.liveSessions.set('session-live', {
    streaming: true,
    text: '正在处理剩余测试…',
    thinkingText: '先检查失败测试，再修复实现。',
    tools: [{ type: 'tool', id: 'tool-1', name: 'bash', args: { command: 'npm test' }, status: 'running' }],
    currentActivity: { type: 'tool', id: 'tool-1', name: 'bash', args: { command: 'npm test' }, status: 'running', updatedAt: '2026-07-20T10:00:05.000Z' },
    activityFeed: [{ type: 'tool', id: 'tool-1', name: 'bash', args: { command: 'npm test' }, status: 'running', updatedAt: '2026-07-20T10:00:05.000Z' }],
    assets: [],
    error: '',
    startedAt: '2026-07-20T10:00:00.000Z',
    lastActivityAt: '2026-07-20T10:00:05.000Z',
  })
  const live = await runtime.getSessionLive('session-live')
  assert.equal(live.streaming, true)
  assert.equal(live.messages.at(-1).role, 'agent')
  assert.equal(live.messages.at(-1).text, '正在处理剩余测试…')
  assert.equal(live.thinkingText, '先检查失败测试，再修复实现。')
  assert.deepEqual(live.tools, [{ type: 'tool', id: 'tool-1', name: 'bash', args: { command: 'npm test' }, status: 'running' }])
  assert.equal(live.currentActivity.args.command, 'npm test')
  assert.equal(live.activityFeed[0].args.command, 'npm test')
  assert.equal(live.startedAt, '2026-07-20T10:00:00.000Z')
  assert.equal(live.lastActivityAt, '2026-07-20T10:00:05.000Z')
  assert.equal(live.model, 'openai/gpt-5.4')
  assert.deepEqual(live.agents, [{ id: 'agent-live', canonicalName: '/root/live_1', status: 'running' }])
})

test('multi-Agent status inspection never promotes an unrelated failed Agent into current activity', () => {
  const failed = { id: 'failed-agent', status: 'failed' }
  assert.equal(multiAgentResultAgent('list_agents', { agents: [failed] }), null)
  assert.equal(multiAgentResultAgent('wait_agent', { agents: [failed], agent: null }), null)
  assert.deepEqual(multiAgentResultAgent('wait_agent', { agents: [failed], agent: failed }), failed)
  assert.deepEqual(multiAgentResultAgent('spawn_agent', failed), failed)
})

test('live activity replaces plan and Agent status without retaining terminal Agent cards', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-live-activity-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose?.().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.liveSessions.set('session-activity', {
    streaming: true,
    agents: [],
    taskList: {
      items: [{ id: 'one', title: 'Implement', status: 'in_progress', assignee: '', dependsOn: [] }],
    },
    currentActivity: { type: 'model', stage: 'thinking' },
  })
  const running = { id: 'agent-running', canonicalName: '/root/running_1', status: 'running', lastActivityAt: '2026-07-20T10:00:01.000Z' }
  const completed = { id: 'agent-completed', canonicalName: '/root/completed_1', status: 'completed', lastActivityAt: '2026-07-20T10:00:02.000Z' }
  runtime.multiAgents = { summaries: () => [running, completed] }

  let update = null
  runtime.emitAgentUpdate('session-activity', completed, (event, data) => { update = { event, data } })
  assert.deepEqual(runtime.liveSessions.get('session-activity').agents, [running])
  assert.equal(runtime.liveSessions.get('session-activity').currentActivity.agent.id, completed.id)
  assert.equal(update.event, 'agent_update')
  assert.deepEqual(update.data.agents, [running])

  const taskList = {
    items: [{ id: 'one', title: 'Implement', status: 'in_progress', assignee: '/root/builder_1', dependsOn: ['research'] }],
    counts: { completed: 0, inProgress: 1 },
    updatedAt: '2026-07-20T10:00:03.000Z',
  }
  runtime.emitTaskListUpdate('session-activity', taskList, (event, data) => { update = { event, data } })
  assert.equal(runtime.liveSessions.get('session-activity').currentActivity.type, 'plan')
  assert.equal(runtime.liveSessions.get('session-activity').activityFeed.at(-1).changes[0].title, 'Implement')
  assert.equal(runtime.liveSessions.get('session-activity').activityFeed.at(-1).changes[0].kind, 'updated')
  assert.equal(update.event, 'task_list_update')
  assert.equal(update.data.currentActivity.taskList, taskList)
})

test('wait_agent consumes the terminal mailbox item without starting a parent turn', async () => {
  const acknowledged = []
  const agent = {
    id: 'agent-terminal',
    canonicalName: '/root/review_1',
    status: 'failed',
    message: 'Review the code.',
    error: 'Review failed.',
    resultVersion: 1,
  }
  const multiAgents = {
    wait: async () => ({ timedOut: false, agents: [agent], agent }),
    acknowledge: async (sessionId, agents) => acknowledged.push({ sessionId, agents }),
  }

  const result = await waitForAgentMailbox(multiAgents, 'session-parent', 15_000, agent.id)

  assert.equal(result.agent, agent)
  assert.deepEqual(acknowledged, [{ sessionId: 'session-parent', agents: [agent] }])

  multiAgents.wait = async () => ({ timedOut: true, agents: [agent], agent: null })
  await waitForAgentMailbox(multiAgents, 'session-parent', 250)
  assert.equal(acknowledged.length, 1)
})

test('stream completion publishes an authoritative terminal snapshot', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-live-terminal-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  const listeners = new Set()
  const events = []
  const streamedText = (eventName) => events
    .filter((item) => item.event === eventName)
    .reduce((text, item) => applyTextPatch(text, item.data), '')
  let thinkingAtBlockEnd = ''
  let textAtBlockEnd = ''
  const session = {
    sessionId: 'session-terminal',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier context', timestamp: 1 }],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async prompt() {
      session.isStreaming = true
      for (const listener of listeners) listener({ type: 'compaction_start', reason: 'threshold' })
      for (const listener of listeners) listener({
        type: 'compaction_end',
        reason: 'threshold',
        result: { summary: 'redacted from the public event', firstKeptEntryId: 'message-1', tokensBefore: 92_000, estimatedTokensAfter: 18_500 },
        aborted: false,
        willRetry: false,
      })
      for (const listener of listeners) listener({ type: 'message_update', assistantMessageEvent: { type: 'thinking_start', contentIndex: 0 } })
      for (const listener of listeners) listener({ type: 'message_update', assistantMessageEvent: { type: 'thinking_delta', contentIndex: 0, delta: 'Inspecting the remaining tests before reading files.' } })
      for (const listener of listeners) listener({ type: 'message_update', assistantMessageEvent: { type: 'thinking_end', contentIndex: 0 } })
      thinkingAtBlockEnd = streamedText('thinking_patch')
      for (const listener of listeners) listener({ type: 'tool_execution_start', toolCallId: 'tool-1', toolName: 'bash', args: { command: 'npm test' } })
      for (const listener of listeners) listener({
        type: 'tool_execution_update',
        toolCallId: 'tool-1',
        toolName: 'bash',
        partialResult: { content: [{ type: 'text', text: '\u001b[32mfirst line\u001b[0m\nsecond line' }] },
      })
      for (const listener of listeners) listener({
        type: 'tool_execution_end',
        toolCallId: 'tool-1',
        toolName: 'bash',
        result: { content: [{ type: 'text', text: '\u001b[32mfirst line\u001b[0m\nsecond line\ncomplete' }] },
        isError: false,
      })
      const assistant = { role: 'assistant', content: [{ type: 'text', text: 'Final answer' }], stopReason: 'stop', timestamp: 2 }
      session.messages.push(assistant)
      for (const listener of listeners) listener({ type: 'message_update', assistantMessageEvent: { type: 'text_start', contentIndex: 0 } })
      for (const listener of listeners) listener({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', contentIndex: 0, delta: 'Final answer' } })
      for (const listener of listeners) listener({ type: 'message_update', assistantMessageEvent: { type: 'text_end', contentIndex: 0 } })
      textAtBlockEnd = streamedText('text_patch')
      for (const listener of listeners) listener({ type: 'message_end', message: assistant })
      session.isStreaming = false
    },
  }
  const value = { session, cwd: directory, name: 'Terminal snapshot', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Finish the answer.',
    send: (event, data) => events.push({ event, data }),
  })

  const compactionStart = events.find((item) => item.event === 'compaction_start')?.data
  const compactionEnd = events.find((item) => item.event === 'compaction_end')?.data
  assert.equal(compactionStart.status, 'running')
  assert.equal(compactionStart.reason, 'threshold')
  assert.equal(compactionEnd.status, 'completed')
  assert.equal(compactionEnd.tokensBefore, 92_000)
  assert.equal(compactionEnd.estimatedTokensAfter, 18_500)
  assert.equal(compactionEnd.tokensSaved, 73_500)
  assert.equal(Object.hasOwn(compactionEnd, 'summary'), false)
  let thinkingText = ''
  for (const item of events.filter((event) => event.event === 'thinking_patch')) thinkingText = applyTextPatch(thinkingText, item.data)
  assert.equal(thinkingAtBlockEnd, 'Inspecting the remaining tests before reading files.')
  assert.equal(thinkingText, thinkingAtBlockEnd)
  assert.equal(textAtBlockEnd, 'Final answer')
  const textEndEvents = events.filter((item) => item.event === 'text_end')
  assert.ok(textEndEvents.some((item) => item.data.text === 'Final answer'))
  assert.ok(textEndEvents.some((item) => item.data.final === true))
  const toolUpdate = events.find((item) => item.event === 'tool_update')?.data
  const toolEnd = events.find((item) => item.event === 'tool_end')?.data
  assert.equal(toolUpdate.output, '\u001b[32mfirst line\u001b[0m\nsecond line')
  assert.equal(toolEnd.output, '\u001b[32mfirst line\u001b[0m\nsecond line\ncomplete')
  const done = events.find((item) => item.event === 'done')?.data
  assert.equal(done.text, 'Final answer')
  assert.equal(done.tools[0].status, 'done')
  assert.equal(done.tools[0].output, toolEnd.output)
  assert.deepEqual(done.compaction, compactionEnd)
  assert.ok(done.finishedAt)
  const live = await runtime.getSessionLive(session.sessionId)
  assert.equal(live.streaming, false)
  assert.equal(live.finishedAt, done.finishedAt)
  assert.equal(live.tools[0].status, 'done')
  assert.equal(live.currentActivity, null)
  assert.deepEqual(live.compaction, compactionEnd)
})

test('background memory candidate extraction never blocks or delays session completion', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-memory-background-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  const timeline = []
  runtime.captureConversationMemory = async () => {
    timeline.push('candidate-extraction-started')
    await new Promise(() => {})
  }

  const listeners = new Set()
  const session = {
    sessionId: 'session-memory-background',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier turn', timestamp: 1 }],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async prompt() {
      session.isStreaming = true
      session.messages.push({ role: 'assistant', content: [{ type: 'text', text: 'Current task completed.' }], timestamp: 2 })
      session.isStreaming = false
    },
  }
  const value = { session, cwd: directory, name: 'Background memory', baseToolNames: [], enabledTools: ['memory_remember'] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Remember this preference and finish the current task.',
    send: (event) => timeline.push(event),
  })

  assert.ok(timeline.includes('done'))
  assert.ok(timeline.indexOf('done') < timeline.indexOf('candidate-extraction-started'))
})

test('terminal Agent results enter an active parent loop before its next model call', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-agent-mailbox-active-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  let mailbox = []
  runtime.multiAgents = {
    summaries: () => [],
    peekMailbox: () => mailbox.map((agent) => ({ ...agent })),
    acknowledge: async (_sessionId, agents) => {
      const ids = new Set(agents.map((agent) => agent.mailboxId))
      mailbox = mailbox.filter((agent) => !ids.has(agent.mailboxId))
      return true
    },
  }

  const listeners = new Set()
  let releasePrompt
  let promptStarted
  const started = new Promise((resolve) => { promptStarted = resolve })
  const release = new Promise((resolve) => { releasePrompt = resolve })
  let mainPrompt = ''
  let customDelivery = null
  let promptCalls = 0
  const session = {
    sessionId: 'session-mailbox-active',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier context', timestamp: 1 }],
    agent: { state: { systemPrompt: 'Base prompt' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async prompt(prompt) {
      promptCalls += 1
      mainPrompt = prompt
      session.isStreaming = true
      promptStarted()
      await release
      session.messages.push({ role: 'assistant', content: [{ type: 'text', text: 'Combined the background result.' }], timestamp: 3 })
      session.isStreaming = false
    },
    async sendCustomMessage(message, options) {
      customDelivery = { message, options, wasStreaming: session.isStreaming }
      const appMessage = { role: 'custom', ...message, timestamp: 2 }
      session.messages.push(appMessage)
      for (const listener of listeners) listener({ type: 'message_start', message: appMessage })
      for (const listener of listeners) listener({ type: 'message_end', message: appMessage })
    },
  }
  const value = { session, cwd: directory, name: 'Active mailbox', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const run = runtime.streamPrompt({ sessionId: session.sessionId, message: 'Continue the parent task.', send: () => {} })
  await started
  mailbox.push({
    id: 'agent-active',
    mailboxId: 'agent-active:1',
    canonicalName: '/root/review_active_1',
    parentSessionId: session.sessionId,
    status: 'completed',
    message: 'Review the runtime.',
    output: 'Found an active-loop race.',
    error: '',
    resultVersion: 1,
  })
  const delivered = await runtime.deliverAgentMailboxToSession(session.sessionId)

  assert.equal(delivered, true)
  assert.equal(customDelivery.wasStreaming, true)
  assert.equal(customDelivery.options.deliverAs, 'steer')
  assert.equal(customDelivery.options.triggerTurn, false)
  assert.equal(customDelivery.message.display, false)
  assert.match(customDelivery.message.content, /Found an active-loop race/)
  assert.doesNotMatch(mainPrompt, /Found an active-loop race/)
  assert.equal(mailbox.length, 0)
  assert.equal(promptCalls, 1)

  releasePrompt()
  await run
  assert.equal(promptCalls, 1)
})

test('terminal Agent results do not start a turn when the loaded parent is idle', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-agent-mailbox-idle-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  let mailbox = [{
    id: 'agent-idle',
    mailboxId: 'agent-idle:1',
    canonicalName: '/root/review_idle_1',
    parentSessionId: 'session-mailbox-idle',
    status: 'completed',
    message: 'Review while the parent is idle.',
    output: 'Idle result.',
    error: '',
    resultVersion: 1,
  }]
  runtime.multiAgents = {
    peekMailbox: () => mailbox.map((agent) => ({ ...agent })),
    acknowledge: async (_sessionId, agents) => {
      const ids = new Set(agents.map((agent) => agent.mailboxId))
      mailbox = mailbox.filter((agent) => !ids.has(agent.mailboxId))
      return true
    },
  }
  let customDelivery = null
  let promptCalls = 0
  const session = {
    sessionId: 'session-mailbox-idle',
    isStreaming: false,
    messages: [],
    async prompt() { promptCalls += 1 },
    async sendCustomMessage(message, options) {
      customDelivery = { message, options }
      session.messages.push({ role: 'custom', ...message, timestamp: 1 })
    },
  }
  runtime.sessions.set(session.sessionId, { session })

  const delivered = await runtime.deliverAgentMailboxToSession(session.sessionId)

  assert.equal(delivered, true)
  assert.equal(customDelivery.options.deliverAs, 'steer')
  assert.equal(customDelivery.options.triggerTurn, false)
  assert.match(customDelivery.message.content, /Idle result/)
  assert.equal(promptCalls, 0)
  assert.equal(mailbox.length, 0)
})

test('unread background Agent results use custom context without rewriting the next user prompt', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-agent-mailbox-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  const mailbox = [{
    id: 'agent-1',
    mailboxId: 'agent-1:1',
    canonicalName: '/root/review_1',
    parentSessionId: 'session-mailbox',
    status: 'completed',
    message: 'Review the runtime.',
    output: 'Found a race in startup handling.',
    error: '',
    resultVersion: 1,
    deliveredVersion: 0,
  }]
  let acknowledged = null
  runtime.multiAgents = {
    summaries: () => mailbox,
    peekMailbox: () => mailbox,
    acknowledge: async (sessionId, agents) => { acknowledged = { sessionId, agents } },
  }

  const listeners = new Set()
  let observedPrompt = ''
  let customDelivery = null
  const session = {
    sessionId: 'session-mailbox',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier context', timestamp: 1 }],
    agent: { state: { systemPrompt: 'Base prompt' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async prompt(prompt) {
      session.isStreaming = true
      observedPrompt = prompt
      session.messages.push({ role: 'assistant', content: [{ type: 'text', text: 'Used the background result.' }], timestamp: 2 })
      session.isStreaming = false
    },
    async sendCustomMessage(message, options) {
      customDelivery = { message, options }
      session.messages.push({ role: 'custom', ...message, timestamp: 2 })
    },
  }
  const value = { session, cwd: directory, name: 'Mailbox', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Continue.',
    send: () => {},
  })

  assert.equal(observedPrompt, 'Continue.')
  assert.match(customDelivery.message.content, /vesper_agent_mailbox_results/)
  assert.match(customDelivery.message.content, /Found a race in startup handling/)
  assert.equal(customDelivery.message.display, false)
  assert.equal(customDelivery.options.triggerTurn, false)
  assert.match(session.agent.state.systemPrompt, /^Base prompt/)
  assert.doesNotMatch(session.agent.state.systemPrompt, /vesper_agent_mailbox/)
  assert.equal(acknowledged.sessionId, session.sessionId)
  assert.equal(acknowledged.agents[0].mailboxId, 'agent-1:1')
})

test('Agent mailbox versions arriving during a parent turn remain queued for the next turn', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-agent-mailbox-race-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  const record = {
    id: 'agent-race',
    canonicalName: '/root/review_race_1',
    parentSessionId: 'session-mailbox-race',
    status: 'completed',
    message: 'Review the runtime.',
    output: 'First result.',
    error: '',
    resultVersion: 1,
    deliveredVersion: 0,
  }
  runtime.multiAgents = {
    summaries: () => [{ ...record }],
    peekMailbox: () => record.resultVersion > record.deliveredVersion
      ? [{ ...record, mailboxId: `${record.id}:${record.resultVersion}` }]
      : [],
    acknowledge: async (_sessionId, agents) => {
      for (const agent of agents) {
        const resultVersion = Number(agent.resultVersion || String(agent.mailboxId || '').split(':').at(-1))
        record.deliveredVersion = Math.max(record.deliveredVersion, resultVersion)
      }
    },
  }

  const listeners = new Set()
  const observedPrompts = []
  const customDeliveries = []
  let promptCount = 0
  const session = {
    sessionId: 'session-mailbox-race',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier context', timestamp: 1 }],
    agent: { state: { systemPrompt: 'Base prompt' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async prompt(prompt) {
      promptCount += 1
      observedPrompts.push(prompt)
      session.isStreaming = true
      if (promptCount === 1) {
        record.output = 'Second result that arrived during the parent turn.'
        record.resultVersion = 2
      }
      session.messages.push({ role: 'assistant', content: [{ type: 'text', text: `Parent answer ${promptCount}` }], timestamp: promptCount + 1 })
      session.isStreaming = false
    },
    async sendCustomMessage(message) {
      customDeliveries.push(message)
      session.messages.push({ role: 'custom', ...message, timestamp: customDeliveries.length + 1 })
    },
  }
  const value = { session, cwd: directory, name: 'Mailbox race', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  await runtime.streamPrompt({ sessionId: session.sessionId, message: 'First parent turn.', send: () => {} })
  assert.equal(observedPrompts[0], 'First parent turn.')
  assert.match(customDeliveries[0].content, /First result/)
  assert.equal(record.deliveredVersion, 1)
  assert.equal(record.resultVersion, 2)

  await runtime.streamPrompt({ sessionId: session.sessionId, message: 'Second parent turn.', send: () => {} })
  assert.equal(observedPrompts[1], 'Second parent turn.')
  assert.match(customDeliveries[1].content, /Second result that arrived during the parent turn/)
  assert.equal(record.deliveredVersion, 2)
})

test('generated session title is emitted before the terminal done event', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-live-title-order-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  runtime.generateSessionTitle = async () => 'Generated Title'
  runtime.markSessionTitle = async () => {}

  const listeners = new Set()
  const session = {
    sessionId: 'session-title-order',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    setSessionName(name) { this.name = name },
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async prompt() {
      session.isStreaming = true
      session.messages.push({ role: 'user', content: 'Name this chat.', timestamp: 1 })
      session.messages.push({ role: 'assistant', content: [{ type: 'text', text: 'Hello' }], timestamp: 2 })
      for (const listener of listeners) listener({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'Hello' } })
      session.isStreaming = false
    },
  }
  const value = { session, cwd: directory, name: 'Temporary title', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Name this chat.',
    send: (event, data) => events.push({ event, data }),
  })

  const titleIndex = events.findIndex((item) => item.event === 'session_title' && item.data?.source === 'generated')
  const doneIndex = events.findIndex((item) => item.event === 'done')
  assert.ok(titleIndex >= 0)
  assert.ok(doneIndex > titleIndex)
  assert.equal(events[titleIndex].data.name, 'Generated Title')
})

test('stream failures emit a single terminal error snapshot without throwing', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-live-error-once-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  const session = {
    sessionId: 'session-error-once',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier', timestamp: 1 }],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe() { return () => {} },
    async prompt() {
      session.isStreaming = true
      try {
        throw new Error('model failed')
      } finally {
        session.isStreaming = false
      }
    },
  }
  const value = { session, cwd: directory, name: 'Error once', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Trigger failure.',
    send: (event, data) => events.push({ event, data }),
  })

  const errors = events.filter((item) => item.event === 'error')
  assert.equal(errors.length, 1)
  assert.equal(errors[0].data.message, 'model failed')
  assert.equal(errors[0].data.tools.length, 0)
  const live = await runtime.getSessionLive(session.sessionId)
  assert.equal(live.streaming, false)
  assert.equal(live.error, 'model failed')
})

test('context usage reports the current window share and earlier automatic compaction threshold', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-context-usage-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose?.().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = { getCompactionSettings: () => ({ enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 }) }
  const session = {
    model: { provider: 'openai', id: 'gpt-5.4', contextWindow: 200_000 },
    getContextUsage: () => ({ tokens: 120_000, contextWindow: 200_000, percent: 60 }),
  }
  assert.deepEqual(runtime.compactionAwareContextUsage(session), {
    tokens: 120_000,
    contextWindow: 200_000,
    percent: 60,
    estimated: false,
    autoCompactEnabled: true,
    compactAtTokens: 160_000,
    compactAtPercent: 80,
  })
  assert.deepEqual(runtime.decorateContextUsage({ tokens: null, contextWindow: 200_000, percent: null }, { status: 'completed', estimatedTokensAfter: 18_500 }), {
    tokens: 18_500,
    contextWindow: 200_000,
    percent: 9.25,
    estimated: true,
    autoCompactEnabled: true,
    compactAtTokens: 160_000,
    compactAtPercent: 80,
  })
})

test('session messages are returned newest-first by bounded cursor pages', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-message-pages-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.sessions.set('session-pages', {
    cwd: directory,
    session: {
      isStreaming: false,
      model: { provider: 'openai', id: 'gpt-5.4', contextWindow: 128_000 },
      getContextUsage: () => ({ tokens: 64_000, contextWindow: 128_000, percent: 50 }),
      messages: Array.from({ length: 95 }, (_, index) => ({
        role: index % 2 ? 'assistant' : 'user',
        content: `message-${index}`,
        timestamp: index + 1,
      })),
    },
  })

  const latest = await runtime.getSessionMessagePage('session-pages', { limit: 20 })
  assert.equal(latest.messages.length, 20)
  assert.equal(latest.messages[0].text, 'message-75')
  assert.equal(latest.messages.at(-1).text, 'message-94')
  assert.deepEqual(latest.pageInfo, { start: 75, end: 95, total: 95, hasMore: true, nextCursor: '75' })
  assert.equal(latest.contextUsage.percent, 50)
  assert.equal(latest.contextUsage.contextWindow, 128_000)

  const older = await runtime.getSessionMessagePage('session-pages', { limit: 20, before: latest.pageInfo.nextCursor })
  assert.equal(older.messages[0].text, 'message-55')
  assert.equal(older.messages.at(-1).text, 'message-74')
  assert.equal(older.pageInfo.nextCursor, '55')

  const oldest = await runtime.getSessionMessagePage('session-pages', { limit: 20, before: 15 })
  assert.equal(oldest.messages.length, 15)
  assert.equal(oldest.messages[0].text, 'message-0')
  assert.equal(oldest.pageInfo.hasMore, false)
  assert.equal(oldest.pageInfo.nextCursor, null)
})

test('session history pagination follows the persisted branch across compaction', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-persisted-history-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'session.jsonl')
  const entries = [
    { type: 'session', version: 3, id: 'session-history', timestamp: '2026-01-01T00:00:00.000Z', cwd: directory },
    { type: 'message', id: 'message-1', parentId: 'session-history', message: { role: 'user', content: 'old-user', timestamp: 1 } },
    { type: 'message', id: 'message-2', parentId: 'message-1', message: { role: 'assistant', content: 'old-agent', timestamp: 2 } },
    { type: 'compaction', id: 'compact-1', parentId: 'message-2', summary: 'compressed context' },
    { type: 'message', id: 'message-3', parentId: 'compact-1', message: { role: 'user', content: 'new-user', timestamp: 3 } },
    { type: 'message', id: 'message-4', parentId: 'message-3', message: { role: 'assistant', content: 'new-agent', timestamp: 4 } },
  ]
  await writeFile(path, `${entries.map((entry) => JSON.stringify(entry)).join('\n')}\n`, 'utf8')
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.findSessionInfo = async () => ({ path })

  const latest = await runtime.getSessionMessagePage('session-history', { limit: 2 })
  assert.deepEqual(latest.messages.map((message) => message.text), ['new-user', 'new-agent'])
  assert.equal(latest.pageInfo.total, 4)
  assert.equal(latest.pageInfo.nextCursor, '2')

  const older = await runtime.getSessionMessagePage('session-history', { limit: 2, before: latest.pageInfo.nextCursor })
  assert.deepEqual(older.messages.map((message) => message.text), ['old-user', 'old-agent'])
  assert.equal(older.pageInfo.hasMore, false)
})

test('session listings do not reopen every inactive history just to resolve its model', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-session-list-memory-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.listStoredSessions = async () => [{
    id: 'session-cached',
    path: join(directory, 'session.jsonl'),
    name: 'Cached session',
    firstMessage: 'hello',
    messageCount: 2,
    cwd: directory,
    created: new Date('2026-01-01T00:00:00.000Z'),
    modified: new Date('2026-01-01T00:01:00.000Z'),
  }]
  runtime.settingsManager = { getGlobalSettings: () => ({ defaultProvider: 'openai', defaultModel: 'gpt-5.4' }) }
  runtime.goals = { get: () => null }
  runtime.taskLists = { get: () => null }
  runtime.multiAgents = { summaries: () => [] }
  runtime.openStoredSession = () => {
    throw new Error('inactive history should not be reparsed')
  }

  const sessions = await runtime.listSessions()
  assert.equal(sessions[0].model, 'openai/gpt-5.4')
})

test('today usage scans only newly appended session bytes', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-usage-scan-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'session.jsonl')
  const now = Date.now()
  const entries = [
    { type: 'session', version: 3, id: 'session-usage', timestamp: new Date(now - 1000).toISOString(), cwd: directory },
    { type: 'message', id: 'assistant-1', parentId: null, timestamp: new Date(now).toISOString(), message: { role: 'assistant', timestamp: now, content: [{ type: 'text', text: 'one' }], usage: { input: 10, output: 5, totalTokens: 15 } } },
  ]
  await writeFile(path, `${entries.map((entry) => JSON.stringify(entry)).join('\n')}\n`, 'utf8')

  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.listStoredSessions = async () => [{ id: 'session-usage', path, modified: new Date(now) }]
  runtime.saveUsageLedger = async () => {}
  runtime.openStoredSession = () => {
    throw new Error('usage scanning must stay incremental')
  }

  const first = await runtime.getTodayUsage()
  const firstOffset = runtime.usageLedger.sessionScans['session-usage'].size
  assert.equal(first.totalTokens, 15)

  const secondEntry = { type: 'message', id: 'assistant-2', parentId: 'assistant-1', timestamp: new Date(now + 1).toISOString(), message: { role: 'assistant', timestamp: now + 1, content: [{ type: 'text', text: 'two' }], usage: { input: 20, output: 10, totalTokens: 30 } } }
  await appendFile(path, `${JSON.stringify(secondEntry)}\n`, 'utf8')
  const second = await runtime.getTodayUsage()
  assert.equal(second.totalTokens, 45)
  assert.ok(runtime.usageLedger.sessionScans['session-usage'].size > firstOffset)
})

test('empty active sessions tolerate a JSONL file that has not been created yet', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'vesper-empty-session-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.sessions.set('session-empty', {
    cwd: directory,
    session: {
      sessionFile: join(directory, 'sessions', 'not-created-yet.jsonl'),
      isStreaming: false,
      model: { provider: 'openai', id: 'gpt-5.4' },
      messages: [],
    },
  })

  const page = await runtime.getSessionMessagePage('session-empty', { limit: 40 })
  assert.deepEqual(page.messages, [])
  assert.deepEqual(page.pageInfo, { start: 0, end: 0, total: 0, hasMore: false, nextCursor: null })
})
