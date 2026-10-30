import assert from 'node:assert/strict'
import { appendFile, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  AgentRuntimeService,
  createSessionWithTransientStreamRetry,
  installTransientStreamRetry,
  multiAgentResultAgent,
  resolveSessionDirectory,
  sessionTitleFromFirstMessage,
  waitForAgentMailbox,
} from '../runtime/agent-runtime.mjs'
import { classifyTeamTaskError } from '../services/team-workflow.mjs'
import { parseBoardViewQuery } from '../http/routes/board.mjs'
import { sessionRuntimeRoutes } from '../http/routes/sessions-runtime.mjs'
import { deriveBoardStatus } from '../../shared/session-board-status.mjs'
import { applyTextPatch } from '../../src/lib/api.ts'
import { shouldRetainClosedSessionState } from '../../src/lib/session-state.ts'

test('embedded mobile sessions repair stale workspace paths before runtime actions', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mobile-session-cwd-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  let saves = 0
  const runtime = {
    cwd: directory,
    sessionMeta: { 'session-stale-cwd': { cwd: '/old-mobile-workspace' } },
    async saveSessionMeta() {
      saves += 1
    },
  }
  const sessionManager = { getCwd: () => '/old-mobile-workspace' }

  const cwd = await resolveSessionDirectory(
    runtime,
    sessionManager,
    'session-stale-cwd',
    'mobile-embedded',
  )

  assert.equal(cwd, directory)
  assert.deepEqual(runtime.sessionMeta['session-stale-cwd'], { cwd: directory })
  assert.equal(saves, 1)
})

test('live session snapshot restores partial assistant output and tool state', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-live-session-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.multiAgents = {
    summaries: () => [
      { id: 'agent-live', canonicalName: '/root/live_1', status: 'running' },
      { id: 'agent-finished', canonicalName: '/root/finished_1', status: 'completed' },
    ],
  }
  runtime.sessions.set('session-live', {
    cwd: directory,
    session: {
      isStreaming: true,
      model: { provider: 'openai', id: 'gpt-5.4' },
      messages: [
        { role: 'user', content: '继续处理', timestamp: 1 },
        {
          role: 'assistant',
          content: [
            { type: 'thinking', thinking: '先检查导入。' },
            { type: 'text', text: '补上 open/stat 导入：' },
            { type: 'toolCall', id: 'tool-0', name: 'read', arguments: { path: 'runtime.mjs' } },
          ],
          stopReason: 'toolUse',
          timestamp: 2,
        },
        {
          role: 'toolResult',
          toolCallId: 'tool-0',
          toolName: 'read',
          content: [{ type: 'text', text: 'source' }],
          isError: false,
          timestamp: 3,
        },
        {
          role: 'assistant',
          content: [
            { type: 'thinking', thinking: '继续修改调用方。' },
            { type: 'text', text: '重写 session-lifecycle.mjs 的调用方：' },
            { type: 'toolCall', id: 'tool-1', name: 'bash', arguments: { command: 'npm test' } },
          ],
          stopReason: 'toolUse',
          timestamp: 4,
        },
      ],
    },
  })
  runtime.liveSessions.set('session-live', {
    streaming: true,
    text: '正在处理剩余测试…',
    thinkingText: '先检查失败测试，再修复实现。',
    tools: [
      {
        type: 'tool',
        id: 'tool-1',
        name: 'bash',
        args: { command: 'npm test' },
        status: 'running',
      },
    ],
    currentActivity: {
      type: 'tool',
      id: 'tool-1',
      name: 'bash',
      args: { command: 'npm test' },
      status: 'running',
      updatedAt: '2026-07-20T10:00:05.000Z',
    },
    activityFeed: [
      {
        type: 'tool',
        id: 'tool-1',
        name: 'bash',
        args: { command: 'npm test' },
        status: 'running',
        updatedAt: '2026-07-20T10:00:05.000Z',
      },
    ],
    assets: [],
    error: '',
    startedAt: '2026-07-20T10:00:00.000Z',
    lastActivityAt: '2026-07-20T10:00:05.000Z',
  })
  const live = await runtime.getSessionLive('session-live')
  assert.equal(live.streaming, true)
  assert.equal(live.configurationBusy, true)
  assert.equal(live.messages.length, 2)
  assert.equal(live.messages.at(-1).role, 'agent')
  assert.equal(live.messages.at(-1).text, '正在处理剩余测试…')
  assert.equal(
    live.messages.some((message) =>
      ['补上 open/stat 导入：', '重写 session-lifecycle.mjs 的调用方：'].includes(message.text),
    ),
    false,
  )
  assert.equal(live.thinkingText, '先检查失败测试，再修复实现。')
  assert.deepEqual(live.tools, [
    { type: 'tool', id: 'tool-1', name: 'bash', args: { command: 'npm test' }, status: 'running' },
  ])
  assert.equal(live.currentActivity.args.command, 'npm test')
  assert.equal(live.activityFeed[0].args.command, 'npm test')
  assert.equal(live.startedAt, '2026-07-20T10:00:00.000Z')
  assert.equal(live.lastActivityAt, '2026-07-20T10:00:05.000Z')
  assert.equal(live.model, 'openai/gpt-5.4')
  assert.deepEqual(live.agents, [
    { id: 'agent-live', canonicalName: '/root/live_1', status: 'running' },
  ])
  const active = runtime.sessions.get('session-live')
  const liveState = runtime.liveSessions.get('session-live')
  active.abortedAt = Date.now()
  active.runActive = true
  liveState.streaming = false
  const stopping = await runtime.getSessionLive('session-live')
  assert.equal(stopping.streaming, false, 'stopping still clears the visible spinner')
  assert.equal(stopping.configurationBusy, true, 'the active run still rejects model changes')
  active.runActive = false
  const draining = await runtime.getSessionLive('session-live')
  assert.equal(
    draining.configurationBusy,
    true,
    'Pi can still be streaming after run cleanup starts',
  )
  active.session.isStreaming = false
  const stopped = await runtime.getSessionLive('session-live')
  assert.equal(stopped.streaming, false)
  assert.equal(stopped.configurationBusy, false, 'teardown makes model changes available')
})

test('session catalog and runtime LRU treat a live tool run as active between Pi stream turns', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-live-session-catalog-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  const id = 'session-live-tool'
  runtime.settingsManager = {
    getGlobalSettings: () => ({
      defaultProvider: 'openai',
      defaultModel: 'gpt-5.4',
      defaultThinkingLevel: 'medium',
    }),
  }
  runtime.listStoredSessions = async () => [
    {
      id,
      path: join(directory, 'session.jsonl'),
      name: 'Live tool run',
      firstMessage: 'Inspect the runtime.',
      messageCount: 1,
      cwd: directory,
      created: new Date('2026-08-09T08:52:52.000Z'),
      modified: new Date('2026-08-09T08:53:08.000Z'),
    },
  ]
  const value = {
    name: 'Live tool run',
    cwd: directory,
    created: '2026-08-09T08:52:52.000Z',
    modified: '2026-08-09T08:53:08.000Z',
    session: {
      isStreaming: false,
      model: { provider: 'openai', id: 'gpt-5.4' },
      thinkingLevel: 'medium',
      messages: [{ role: 'user', content: 'Inspect the runtime.' }],
      dispose() {
        throw new Error('a live run must never be disposed by the LRU')
      },
    },
  }
  runtime.sessions.set(id, value)
  runtime.liveSessions.set(id, { streaming: true })

  const [summary] = await runtime.listSessions()

  assert.equal(summary.streaming, true)
  assert.equal(runtime.sessionRunIsActive(id, value), true)
  assert.equal(runtime.sessionRuntimeIsProtected(id, value), true)
  assert.equal(runtime.disposeSessionRuntime(id, value), false)
  assert.equal(runtime.sessions.get(id), value)
})

test('session catalog hides the Pi empty-session placeholder from names and summaries', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-empty-session-catalog-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = { getGlobalSettings: () => ({}) }
  runtime.listStoredSessions = async () => [
    {
      id: 'session-empty',
      path: join(directory, 'session.jsonl'),
      name: '',
      firstMessage: '(no messages)',
      messageCount: 0,
      cwd: directory,
      created: new Date('2026-10-28T08:00:00.000Z'),
      modified: new Date('2026-10-28T08:00:00.000Z'),
    },
  ]

  const [summary] = await runtime.listSessions()

  assert.equal(summary.firstMessage, '')
  assert.equal(summary.name, '新会话')
})

test('live tool runs block reentrant prompts and session configuration between Pi stream turns', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-live-session-guards-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await writeFile(runtime.appConfigPath, '{}', 'utf8')
  const id = 'session-live-guards'
  const value = {
    session: {
      sessionId: id,
      isStreaming: false,
      model: { provider: 'openai', id: 'gpt-current' },
      thinkingLevel: 'medium',
    },
  }
  runtime.sessions.set(id, value)
  runtime.liveSessions.set(id, { streaming: true })
  runtime.getOrCreateSession = async () => value
  runtime.modelRuntime = {
    getModel: () => ({ provider: 'openai', id: 'gpt-next' }),
  }

  await assert.rejects(
    runtime.providerPreferences.setSessionModel(id, 'openai', 'gpt-next'),
    /当前会话正在运行/,
  )
  await assert.rejects(
    runtime.providerPreferences.setSessionThinkingLevel(id, 'high'),
    /当前会话正在运行/,
  )
  await assert.rejects(runtime.compactSession(id), /当前会话仍在运行/)
  await assert.rejects(
    runtime.streamPrompt({ sessionId: id, message: 'Do not start twice.', send: () => {} }),
    /当前会话仍在运行/,
  )
})

test('persisted transcript binds reasoning and tool activity to the completed Agent run', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-persisted-activity-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.sessions.set('session-activity-history', {
    cwd: directory,
    session: {
      isStreaming: false,
      model: { provider: 'openai', id: 'gpt-5.4' },
      messages: [
        { role: 'user', content: 'Start the task.', timestamp: '2026-07-20T10:00:00.000Z' },
        {
          role: 'assistant',
          content: [
            { type: 'thinking', thinking: 'Inspect the implementation.' },
            { type: 'text', text: '先运行测试。' },
            { type: 'toolCall', id: 'tool-1', name: 'bash', arguments: { command: 'npm test' } },
          ],
          stopReason: 'toolUse',
          timestamp: '2026-07-20T10:00:01.000Z',
        },
        {
          role: 'toolResult',
          toolCallId: 'tool-1',
          toolName: 'bash',
          content: [{ type: 'text', text: 'all tests passed' }],
          isError: false,
          timestamp: '2026-07-20T10:00:02.000Z',
        },
        {
          role: 'user',
          content: 'Also cover the follow-up.',
          timestamp: '2026-07-20T10:00:03.000Z',
        },
        {
          role: 'assistant',
          content: [
            { type: 'thinking', thinking: 'Apply the requested follow-up.' },
            { type: 'text', text: 'First run complete.' },
          ],
          stopReason: 'stop',
          timestamp: '2026-07-20T10:00:04.000Z',
        },
        { role: 'user', content: 'Start a separate round.', timestamp: '2026-07-20T10:01:00.000Z' },
        {
          role: 'assistant',
          content: [
            { type: 'thinking', thinking: 'Handle only the new round.' },
            { type: 'text', text: 'Second run complete.' },
          ],
          stopReason: 'stop',
          timestamp: '2026-07-20T10:01:01.000Z',
        },
        {
          role: 'user',
          content: 'Recover an interrupted run.',
          timestamp: '2026-07-20T10:02:00.000Z',
        },
        {
          role: 'assistant',
          content: [
            { type: 'thinking', thinking: 'Persist activity before the final response.' },
            { type: 'toolCall', id: 'tool-2', name: 'read', arguments: { path: 'package.json' } },
          ],
          stopReason: 'toolUse',
          timestamp: '2026-07-20T10:02:01.000Z',
        },
        {
          role: 'toolResult',
          toolCallId: 'tool-2',
          toolName: 'read',
          content: [{ type: 'text', text: '{ "name": "pisper" }' }],
          isError: false,
          timestamp: '2026-07-20T10:02:02.000Z',
        },
      ],
    },
  })
  const sessionId = 'session-activity-history'
  const path = join(directory, 'session.jsonl')
  const persistedMessages = runtime.sessions.get(sessionId).session.messages
  const entries = [
    {
      type: 'session',
      version: 3,
      id: sessionId,
      timestamp: '2026-07-20T10:00:00.000Z',
      cwd: directory,
    },
    ...persistedMessages.map((message, index) => ({
      type: 'message',
      id: `message-${index + 1}`,
      parentId: index === 0 ? sessionId : `message-${index}`,
      message,
    })),
  ]
  await writeFile(path, `${entries.map((entry) => JSON.stringify(entry)).join('\n')}\n`, 'utf8')
  runtime.sessions.delete(sessionId)
  runtime.findSessionInfo = async () => ({ path })

  const page = await runtime.getSessionMessagePage(sessionId, { limit: 20 })
  assert.deepEqual(
    page.messages.map((message) => message.text),
    [
      'Start the task.',
      'Also cover the follow-up.',
      'First run complete.',
      'Start a separate round.',
      'Second run complete.',
      'Recover an interrupted run.',
      '',
    ],
  )
  assert.equal(
    page.messages.some((message) => message.text === '先运行测试。'),
    false,
  )
  const firstRun = page.messages[2].runActivity
  assert.equal(
    firstRun.thinkingText,
    'Inspect the implementation.\n\nApply the requested follow-up.',
  )
  assert.equal(firstRun.tools[0].name, 'bash')
  assert.equal(firstRun.tools[0].status, 'done')
  assert.equal(firstRun.tools[0].output, 'all tests passed')
  assert.equal(page.messages[2].turnBoundaryEntryId, 'message-5')
  assert.equal(page.messages[4].runActivity.thinkingText, 'Handle only the new round.')
  assert.equal(page.messages[4].runActivity.tools.length, 0)
  assert.equal(page.messages[4].turnBoundaryEntryId, 'message-7')
  assert.equal(page.messages[6].turnBoundaryEntryId, undefined)
  assert.equal(
    page.messages[6].runActivity.thinkingText,
    'Persist activity before the final response.',
  )
  assert.equal(page.messages[6].runActivity.tools[0].name, 'read')
  assert.equal(page.messages[6].runActivity.tools[0].status, 'done')
})

test('persisted transcript settles orphaned tools from a terminated turn', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-terminated-tool-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.sessions.set('session-terminated-tool', {
    cwd: directory,
    session: {
      isStreaming: false,
      messages: [
        { role: 'user', content: 'Attempt an edit.', timestamp: '2026-07-20T10:03:00.000Z' },
        {
          role: 'assistant',
          content: [
            { type: 'thinking', thinking: 'Attempt the edit before the stream terminates.' },
            {
              type: 'toolCall',
              id: 'tool-3',
              name: 'edit',
              arguments: { path: 'runtime/runtime.mjs' },
            },
          ],
          stopReason: 'error',
          errorMessage: 'terminated',
          timestamp: '2026-07-20T10:03:01.000Z',
        },
      ],
    },
  })

  const page = await runtime.getSessionMessagePage('session-terminated-tool', { limit: 10 })
  const terminatedRun = page.messages[1].runActivity
  assert.equal(terminatedRun.tools[0].name, 'edit')
  assert.equal(terminatedRun.tools[0].status, 'error')
  assert.equal(terminatedRun.tools[0].message, 'terminated')
  assert.equal(terminatedRun.tools[0].finishedAt, '2026-07-20T10:03:01.000Z')
})

test('multi-Agent status inspection never promotes an unrelated failed Agent into current activity', () => {
  const failed = { id: 'failed-agent', status: 'failed' }
  assert.equal(multiAgentResultAgent('list_agents', { agents: [failed] }), null)
  assert.equal(multiAgentResultAgent('wait_agent', { agents: [failed], agent: null }), null)
  assert.deepEqual(multiAgentResultAgent('wait_agent', { agents: [failed], agent: failed }), failed)
  assert.deepEqual(multiAgentResultAgent('spawn_agent', failed), failed)
})

test('live activity replaces plan and Agent status without retaining terminal Agent cards', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-live-activity-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose?.().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.liveSessions.set('session-activity', {
    streaming: true,
    agents: [],
    plan: {
      items: [
        { id: 'one', title: 'Implement', status: 'in_progress', assignee: '', dependsOn: [] },
      ],
    },
    currentActivity: { type: 'model', stage: 'thinking' },
  })
  const running = {
    id: 'agent-running',
    canonicalName: '/root/running_1',
    status: 'running',
    lastActivityAt: '2026-07-20T10:00:01.000Z',
  }
  const completed = {
    id: 'agent-completed',
    canonicalName: '/root/completed_1',
    status: 'completed',
    lastActivityAt: '2026-07-20T10:00:02.000Z',
  }
  runtime.multiAgents = { summaries: () => [running, completed] }

  let update = null
  runtime.emitAgentUpdate('session-activity', completed, (event, data) => {
    update = { event, data }
  })
  assert.deepEqual(runtime.liveSessions.get('session-activity').agents, [running])
  assert.equal(runtime.liveSessions.get('session-activity').currentActivity.agent.id, completed.id)
  assert.equal(update.event, 'agent_update')
  assert.deepEqual(update.data.agents, [running])

  const plan = {
    items: [
      {
        id: 'one',
        title: 'Implement',
        status: 'in_progress',
        assignee: '/root/builder_1',
        dependsOn: ['research'],
      },
    ],
    counts: { completed: 0, inProgress: 1 },
    updatedAt: '2026-07-20T10:00:03.000Z',
  }
  runtime.emitPlanUpdate('session-activity', plan, (event, data) => {
    update = { event, data }
  })
  assert.equal(runtime.liveSessions.get('session-activity').currentActivity.type, 'plan')
  assert.equal(
    runtime.liveSessions.get('session-activity').activityFeed.at(-1).changes[0].title,
    'Implement',
  )
  assert.equal(
    runtime.liveSessions.get('session-activity').activityFeed.at(-1).changes[0].kind,
    'updated',
  )
  assert.equal(update.event, 'plan_update')
  assert.equal(update.data.plan, plan)
  assert.equal(update.data.currentActivity.plan, plan)
  assert.equal(Object.hasOwn(update.data, 'taskList'), false)
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
  const directory = await mkdtemp(join(tmpdir(), 'pisper-live-terminal-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  const listeners = new Set()
  const events = []
  const streamedText = (eventName) =>
    events
      .filter((item) => item.event === eventName)
      .reduce((text, item) => applyTextPatch(text, item.data), '')
  const streamedResponse = () =>
    events.reduce((text, item) => {
      if (item.event === 'text_patch') return applyTextPatch(text, item.data)
      if (item.event === 'text_delta') return text + String(item.data.delta || '')
      if (item.event === 'text_end' && typeof item.data.text === 'string') return item.data.text
      return text
    }, '')
  let thinkingAtBlockEnd = ''
  let commentaryAtBlockEnd = ''
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
      for (const listener of listeners) listener({ type: 'agent_start' })
      for (const listener of listeners) listener({ type: 'compaction_start', reason: 'threshold' })
      for (const listener of listeners)
        listener({
          type: 'compaction_end',
          reason: 'threshold',
          result: {
            summary: 'redacted from the public event',
            firstKeptEntryId: 'message-1',
            tokensBefore: 92_000,
            estimatedTokensAfter: 18_500,
          },
          aborted: false,
          willRetry: false,
        })
      for (const listener of listeners) listener({ type: 'turn_start' })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'thinking_start', contentIndex: 0 },
        })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: {
            type: 'thinking_delta',
            contentIndex: 0,
            delta: 'Inspecting the remaining tests before reading files.',
          },
        })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'thinking_end', contentIndex: 0 },
        })
      thinkingAtBlockEnd = streamedText('thinking_patch')
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'text_start', contentIndex: 1 },
        })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: {
            type: 'text_delta',
            contentIndex: 1,
            delta: 'I will inspect the test output first.',
          },
        })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'text_end', contentIndex: 1 },
        })
      commentaryAtBlockEnd = streamedResponse()
      for (const listener of listeners)
        listener({
          type: 'tool_execution_start',
          toolCallId: 'tool-1',
          toolName: 'bash',
          args: { command: 'npm test', apiKey: 'local-chat-value' },
        })
      for (const listener of listeners)
        listener({
          type: 'tool_execution_update',
          toolCallId: 'tool-1',
          toolName: 'bash',
          partialResult: {
            content: [{ type: 'text', text: '\u001b[32mfirst line\u001b[0m\nsecond line' }],
          },
        })
      for (const listener of listeners)
        listener({
          type: 'tool_execution_end',
          toolCallId: 'tool-1',
          toolName: 'bash',
          result: {
            content: [
              { type: 'text', text: '\u001b[32mfirst line\u001b[0m\nsecond line\ncomplete' },
            ],
          },
          isError: false,
        })
      for (const listener of listeners) listener({ type: 'turn_start' })
      for (const listener of listeners)
        listener({
          type: 'message_start',
          message: { role: 'assistant', content: [], timestamp: 2 },
        })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'thinking_start', contentIndex: 0 },
        })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: {
            type: 'thinking_delta',
            contentIndex: 0,
            delta: 'Applying the appended guidance.',
          },
        })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'thinking_end', contentIndex: 0 },
        })
      const assistant = {
        role: 'assistant',
        content: [{ type: 'text', text: 'Final answer' }],
        stopReason: 'stop',
        timestamp: 2,
      }
      session.messages.push(assistant)
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'text_start', contentIndex: 0 },
        })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'text_delta', contentIndex: 0, delta: 'Final answer' },
        })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'text_end', contentIndex: 0 },
        })
      textAtBlockEnd = streamedResponse()
      for (const listener of listeners) listener({ type: 'message_end', message: assistant })
      for (const listener of listeners)
        listener({ type: 'turn_end', message: assistant, toolResults: [] })
      for (const listener of listeners)
        listener({ type: 'agent_end', messages: [assistant], willRetry: false })
      for (const listener of listeners) listener({ type: 'agent_settled' })
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

  assert.equal(runtime.sessionMeta[session.sessionId].organization.unread, true)
  assert.equal(runtime.sessionMeta[session.sessionId].organization.failed, false)
  assert.ok(runtime.sessionMeta[session.sessionId].organization.lastCompletedAt)

  const meta = events.find((item) => item.event === 'meta')?.data
  assert.equal(meta.plan.sessionId, session.sessionId)
  assert.equal(meta.lifecycle.event, 'prompt_submitted')
  assert.equal(meta.sessionTreeRevision, 0)
  assert.equal(Object.hasOwn(meta, 'taskList'), false)
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
  for (const item of events) {
    if (item.event === 'thinking_reset') thinkingText = String(item.data.thinkingText || '')
    if (item.event === 'thinking_patch') thinkingText = applyTextPatch(thinkingText, item.data)
  }
  assert.equal(thinkingAtBlockEnd, 'Inspecting the remaining tests before reading files.')
  assert.equal(thinkingText, `${thinkingAtBlockEnd}\n\nApplying the appended guidance.`)
  const thinkingResets = events.filter((item) => item.event === 'thinking_reset')
  assert.equal(thinkingResets[1].data.thinkingText, thinkingAtBlockEnd)
  assert.equal(commentaryAtBlockEnd, 'I will inspect the test output first.')
  assert.equal(textAtBlockEnd, 'Final answer')
  assert.deepEqual(
    events.filter((item) => item.event === 'text_patch').map((item) => item.data),
    [
      {
        start: 0,
        text: '',
        updatedAt: events.find((item) => item.event === 'text_patch').data.updatedAt,
      },
    ],
  )
  const textEndEvents = events.filter((item) => item.event === 'text_end')
  assert.ok(textEndEvents.some((item) => item.data.text === 'Final answer'))
  assert.ok(textEndEvents.some((item) => item.data.final === true))
  const lifecycleEvents = events
    .filter((item) => item.event === 'agent_lifecycle')
    .map((item) => item.data.lifecycle.event)
  assert.ok(lifecycleEvents.includes('agent_start'))
  assert.ok(lifecycleEvents.includes('message_start'))
  assert.ok(lifecycleEvents.includes('turn_end'))
  assert.ok(lifecycleEvents.includes('agent_settled'))
  const toolStart = events.find((item) => item.event === 'tool_start')?.data
  const toolUpdate = events.find((item) => item.event === 'tool_update')?.data
  const toolEnd = events.find((item) => item.event === 'tool_end')?.data
  assert.equal(toolStart.args.apiKey, 'local-chat-value')
  assert.equal(toolUpdate.output, '\u001b[32mfirst line\u001b[0m\nsecond line')
  assert.equal(toolEnd.output, '\u001b[32mfirst line\u001b[0m\nsecond line\ncomplete')
  const done = events.find((item) => item.event === 'done')?.data
  assert.equal(done.text, 'Final answer')
  assert.equal(done.lifecycle.event, 'runtime_done')
  assert.equal(done.lifecycle.turn, 2)
  assert.equal(done.sessionTreeRevision, 0)
  assert.equal(done.tools[0].status, 'done')
  assert.equal(done.tools[0].output, toolEnd.output)
  assert.deepEqual(done.compaction, compactionEnd)
  assert.equal(done.plan.sessionId, session.sessionId)
  assert.equal(Object.hasOwn(done, 'taskList'), false)
  assert.ok(done.finishedAt)
  const live = await runtime.getSessionLive(session.sessionId)
  assert.equal(live.streaming, false)
  assert.equal(live.finishedAt, done.finishedAt)
  assert.equal(live.lifecycle.phase, 'completed')
  assert.equal(live.lifecycle.event, 'runtime_done')
  assert.equal(live.lifecycle.turn, 2)
  assert.equal(live.tools[0].status, 'done')
  assert.equal(live.currentActivity, null)
  assert.equal(live.plan.sessionId, session.sessionId)
  assert.equal(Object.hasOwn(live, 'taskList'), false)
  assert.equal(live.thinkingText, `${thinkingAtBlockEnd}\n\nApplying the appended guidance.`)
  assert.deepEqual(live.compaction, compactionEnd)
})

test('background memory candidate extraction never blocks or delays session completion', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-memory-background-'))
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
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Current task completed.' }],
        timestamp: 2,
      })
      session.isStreaming = false
    },
  }
  const value = {
    session,
    cwd: directory,
    name: 'Background memory',
    baseToolNames: [],
    enabledTools: ['memory_remember'],
  }
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

test('image attachments reach the Pi prompt when the selected model supports images', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-image-prompt-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => [{ path: join(directory, 'image.png') }]
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  let observedPrompt = ''
  let observedOptions
  const session = {
    sessionId: 'session-image-prompt',
    isStreaming: false,
    model: { provider: 'relay', id: 'gpt-5.6-sol', input: ['text', 'image'] },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier context', timestamp: 1 }],
    agent: { state: { systemPrompt: 'Base prompt' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe() {
      return () => {}
    },
    async prompt(prompt, options) {
      observedPrompt = prompt
      observedOptions = options
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'I can see the image.' }],
        stopReason: 'stop',
        timestamp: 2,
      })
    },
  }
  const value = { session, cwd: directory, name: 'Image prompt', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Analyze this image.',
    attachments: [{ kind: 'image', name: 'image.png', mimeType: 'image/png', data: 'AQID' }],
    send: () => {},
  })

  assert.deepEqual(observedOptions.images, [{ type: 'image', data: 'AQID', mimeType: 'image/png' }])
  assert.match(observedPrompt, /\[Image attachment\] image\.png/)
  assert.match(observedPrompt, /Local path:/)
})

test('local path attachments reach the prompt without reading or archiving file content', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-path-prompt-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  let createAssetCalls = 0
  runtime.createAsset = async () => {
    createAssetCalls += 1
    throw new Error('path attachments must not be archived')
  }
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }

  let observedPrompt = ''
  const session = {
    sessionId: 'session-path-prompt',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [],
    agent: { state: { systemPrompt: 'Base prompt' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    setSessionName: () => {},
    subscribe() {
      return () => {}
    },
    async prompt(prompt) {
      observedPrompt = prompt
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Path received.' }],
        stopReason: 'stop',
        timestamp: 2,
      })
    },
  }
  const value = { session, cwd: directory, name: 'Path prompt', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value
  const path = join(directory, 'arbitrarily-large.bin')

  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Inspect this file when needed.',
    attachments: [
      { kind: 'path', name: 'arbitrarily-large.bin', path, size: Number.MAX_SAFE_INTEGER },
    ],
    send: () => {},
  })

  assert.equal(createAssetCalls, 0)
  assert.match(observedPrompt, /\[Local path attachment\] arbitrarily-large\.bin/)
  assert.match(observedPrompt, new RegExp(path.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))
})

test('background Agent results remain durable without entering parent prompts or custom context', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-agent-mailbox-passive-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  const mailbox = [
    {
      id: 'agent-passive',
      mailboxId: 'agent-passive:1',
      canonicalName: '/root/review_passive_1',
      parentSessionId: 'session-mailbox-passive',
      status: 'completed',
      message: 'Review in the background.',
      output: 'Found a passive mailbox result.',
      error: '',
      resultVersion: 1,
    },
  ]
  let acknowledgeCalls = 0
  runtime.multiAgents = {
    summaries: () => [],
    peekMailbox: () => mailbox.map((agent) => ({ ...agent })),
    acknowledge: async () => {
      acknowledgeCalls += 1
    },
  }

  let observedPrompt = ''
  const session = {
    sessionId: 'session-mailbox-passive',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier context', timestamp: 1 }],
    agent: { state: { systemPrompt: 'Base prompt' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe() {
      return () => {}
    },
    async prompt(prompt) {
      observedPrompt = prompt
      session.isStreaming = true
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Parent finished independently.' }],
        stopReason: 'stop',
        timestamp: 2,
      })
      session.isStreaming = false
    },
  }
  const value = { session, cwd: directory, name: 'Passive mailbox', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Continue the parent task.',
    send: () => {},
  })

  assert.equal(observedPrompt, 'Continue the parent task.')
  assert.equal(
    session.messages.some((message) => message.role === 'custom'),
    false,
  )
  assert.equal(acknowledgeCalls, 0)
  assert.equal(runtime.multiAgents.peekMailbox(session.sessionId).length, 1)
})

test('parent completion snapshot keeps background Agents visible without keeping the parent streaming', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-agent-background-snapshot-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  const background = {
    id: 'agent-background',
    canonicalName: '/root/review_background_1',
    parentSessionId: 'session-agent-background',
    status: 'running',
    message: 'Continue reviewing in the background.',
    startedAt: '2026-07-26T10:00:00.000Z',
    lastActivityAt: '2026-07-26T10:00:01.000Z',
  }
  runtime.multiAgents = {
    summaries: () => [{ ...background }],
    peekMailbox: () => [],
  }

  const session = {
    sessionId: 'session-agent-background',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier context', timestamp: 1 }],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe() {
      return () => {}
    },
    async prompt() {
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Delegated and moved on.' }],
        stopReason: 'stop',
        timestamp: 2,
      })
    },
  }
  const value = { session, cwd: directory, name: 'Background snapshot', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Delegate the review.',
    send: (event, data) => events.push({ event, data }),
  })

  const done = events.find((event) => event.event === 'done')?.data
  assert.equal(done.agents[0].status, 'running')
  assert.equal(done.currentActivity.agent.status, 'running', JSON.stringify(done))
  assert.equal(done.activityFeed[0].agent.id, background.id)
  assert.equal(runtime.liveSessions.get(session.sessionId).streaming, false)
})

test('terminal Agent progress replaces its running activity instead of leaving a stale card', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-agent-terminal-activity-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  const record = {
    id: 'agent-terminal',
    canonicalName: '/root/review_terminal_1',
    status: 'running',
    startedAt: '2026-07-26T10:00:00.000Z',
    lastActivityAt: '2026-07-26T10:00:01.000Z',
  }
  runtime.multiAgents = { summaries: () => [{ ...record }] }
  runtime.liveSessions.set('session-agent-terminal', {
    streaming: false,
    agents: [{ ...record }],
    currentActivity: null,
    activityFeed: [],
  })

  runtime.emitAgentUpdate('session-agent-terminal', record, () => {})
  record.status = 'completed'
  record.completedAt = '2026-07-26T10:00:02.000Z'
  record.lastActivityAt = record.completedAt
  runtime.emitAgentUpdate('session-agent-terminal', record, () => {})

  const live = runtime.liveSessions.get('session-agent-terminal')
  assert.equal(live.activityFeed.length, 1)
  assert.equal(live.activityFeed[0].agent.status, 'completed')
  assert.equal(live.currentActivity.agent.status, 'completed')
  assert.deepEqual(live.agents, [])
})

test('session titles use the first user sentence and truncate locally', () => {
  assert.equal(sessionTitleFromFirstMessage('请修复会话标题。然后运行测试。'), '请修复会话标题')
  assert.equal(sessionTitleFromFirstMessage('Fix foo.ts. Then run tests.'), 'Fix foo.ts')
  assert.equal(sessionTitleFromFirstMessage('# First line\nSecond line'), 'First line')
  assert.equal(sessionTitleFromFirstMessage('', [{ name: 'report.pdf' }]), 'report.pdf')

  const longTitle = '一二三四五六七八九十一二三四五六七八九十甲乙'
  assert.equal(
    sessionTitleFromFirstMessage(longTitle),
    `${Array.from(longTitle).slice(0, 20).join('')}…`,
  )
})

test('the first user sentence is the only automatic title event', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-live-title-order-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  runtime.markSessionTitle = async () => {}
  let titleModelCalls = 0
  runtime.modelRuntime = {
    completeSimple: async () => {
      titleModelCalls += 1
      return { content: [{ type: 'text', text: 'Generated Title' }] }
    },
  }

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
    setSessionName(name) {
      this.name = name
    },
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async prompt() {
      session.isStreaming = true
      session.messages.push({
        role: 'user',
        content: 'Name this chat. Ignore this sentence.',
        timestamp: 1,
      })
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Hello' }],
        timestamp: 2,
      })
      for (const listener of listeners)
        listener({
          type: 'message_update',
          assistantMessageEvent: { type: 'text_delta', delta: 'Hello' },
        })
      session.isStreaming = false
    },
  }
  const value = { session, cwd: directory, name: '新会话', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value

  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Name this chat. Ignore this sentence.',
    send: (event, data) => events.push({ event, data }),
  })

  const titleEvents = events.filter((item) => item.event === 'session_title')
  const titleIndex = events.indexOf(titleEvents[0])
  const doneIndex = events.findIndex((item) => item.event === 'done')
  assert.equal(titleEvents.length, 1)
  assert.equal(titleEvents[0].data.source, 'first_message')
  assert.equal(titleEvents[0].data.name, 'Name this chat')
  assert.ok(doneIndex > titleIndex)
  assert.equal(titleModelCalls, 0)
})

test('upstream stream interruptions use the Pi turn retry path without broadening terminal errors', () => {
  const originalCalls = []
  const session = {
    _isRetryableError(message) {
      originalCalls.push(message)
      return message?.errorMessage === 'existing transient error'
    },
  }

  installTransientStreamRetry(session)
  installTransientStreamRetry(session)

  for (const errorMessage of [
    'stream_read_error',
    'Stream read error: connection closed',
    'OpenAI Responses stream ended before a terminal response event',
    'Error Code internal_server_error: upstream stream closed before a terminal event',
    'upstream_error: Upstream request failed',
    'connection reset by peer (ECONNRESET)',
    'premature close while reading the stream',
    'incomplete stream',
    'unknown: Too many pending requests, please retry later (request id: test)',
  ]) {
    assert.equal(
      session._isRetryableError({ stopReason: 'error', errorMessage }),
      true,
      errorMessage,
    )
  }
  assert.equal(
    session._isRetryableError({ stopReason: 'error', errorMessage: 'invalid api key' }),
    false,
  )
  assert.equal(
    session._isRetryableError({
      stopReason: 'error',
      errorMessage: 'upstream_error: invalid api key',
    }),
    false,
  )
  assert.equal(
    session._isRetryableError({
      stopReason: 'error',
      errorMessage: 'upstream_error: Upstream access forbidden',
    }),
    false,
  )
  assert.equal(
    session._isRetryableError({ stopReason: 'aborted', errorMessage: 'stream_read_error' }),
    false,
  )
  assert.equal(
    session._isRetryableError({ stopReason: 'stop', errorMessage: 'stream_read_error' }),
    false,
  )
  assert.equal(originalCalls.length, 5)
})

test('Team classifies provider queue congestion as retryable upstream work', () => {
  assert.equal(
    classifyTeamTaskError(
      new Error('unknown: Too many pending requests, please retry later (request id: test)'),
      'failed',
    ),
    'upstream_stream',
  )
})

test('child session creation installs the same transient retry policy as the parent', async () => {
  const calls = []
  const childSession = {
    _isRetryableError(message) {
      calls.push(message)
      return false
    },
  }
  const result = await createSessionWithTransientStreamRetry(
    async (options) => {
      assert.equal(options.cwd, '/tmp/team-child')
      return { session: childSession, modelFallbackMessage: '' }
    },
    { cwd: '/tmp/team-child' },
  )

  assert.equal(result.session, childSession)
  assert.equal(
    childSession._isRetryableError({ stopReason: 'error', errorMessage: 'stream_read_error' }),
    true,
  )
  assert.equal(calls.length, 0)
})

test('stream failures emit a single terminal error snapshot without throwing', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-live-error-once-'))
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
    subscribe() {
      return () => {}
    },
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
  assert.equal(errors[0].data.lifecycle.phase, 'failed')
  assert.equal(errors[0].data.lifecycle.event, 'runtime_error')
  assert.equal(errors[0].data.tools.length, 0)
  const live = await runtime.getSessionLive(session.sessionId)
  assert.equal(live.streaming, false)
  assert.equal(live.error, 'model failed')
  assert.equal(live.lifecycle.event, 'runtime_error')
  assert.equal(runtime.sessionMeta[session.sessionId].organization.unread, true)
  assert.equal(runtime.sessionMeta[session.sessionId].organization.failed, true)
})

test('context usage reports the current window share and earlier automatic compaction threshold', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-context-usage-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose?.().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = {
    getCompactionSettings: () => ({
      enabled: true,
      reserveTokens: 16_384,
      keepRecentTokens: 20_000,
    }),
  }
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
  assert.deepEqual(
    runtime.decorateContextUsage(
      { tokens: null, contextWindow: 200_000, percent: null },
      { status: 'completed', estimatedTokensAfter: 18_500 },
    ),
    {
      tokens: 18_500,
      contextWindow: 200_000,
      percent: 9.25,
      estimated: true,
      autoCompactEnabled: true,
      compactAtTokens: 160_000,
      compactAtPercent: 80,
    },
  )
})

test('manual compaction persists through the Agent session and returns safe live state', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-manual-compaction-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  const calls = []
  const session = {
    sessionId: 'session-compact',
    isStreaming: false,
    messages: [{ role: 'user', content: 'older context' }],
    model: { provider: 'openai', id: 'gpt-5.4', contextWindow: 10_000 },
    getContextUsage: () => ({ tokens: 200, contextWindow: 10_000, percent: 2 }),
    async compact() {
      calls.push('compact')
      return {
        summary: 'must not leave the runtime boundary',
        firstKeptEntryId: 'entry-2',
        tokensBefore: 1_000,
        estimatedTokensAfter: 200,
        details: {},
      }
    },
  }
  const value = { session, modified: '' }
  runtime.getOrCreateSession = async (id) => {
    assert.equal(id, session.sessionId)
    return value
  }

  const result = await runtime.compactSession(session.sessionId)

  assert.deepEqual(calls, ['compact'])
  assert.equal(result.compaction.status, 'completed')
  assert.equal(result.compaction.reason, 'manual')
  assert.equal(result.compaction.tokensSaved, 800)
  assert.equal(Object.hasOwn(result, 'summary'), false)
  assert.equal(Object.hasOwn(result.compaction, 'summary'), false)
  assert.equal(result.contextUsage.percent, 2)
  assert.ok(value.modified)

  session.isStreaming = true
  await assert.rejects(runtime.compactSession(session.sessionId), /仍在运行/)
})

test('compaction threshold defaults to 80% and persists valid updates', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-compaction-setting-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })

  assert.equal(runtime.getCompactionPreference().thresholdPercent, 80)
  assert.deepEqual(await runtime.updateCompactionPreference({ thresholdPercent: 72 }), {
    thresholdPercent: 72,
    minPercent: 10,
    maxPercent: 95,
  })
  assert.equal(
    JSON.parse(await readFile(join(directory, 'pisper.json'), 'utf8')).compactionThresholdPercent,
    72,
  )
  await assert.rejects(runtime.updateCompactionPreference({ thresholdPercent: 99 }), /10%.*95%/)
})

test('session messages are returned newest-first by bounded cursor pages', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-message-pages-'))
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
  assert.deepEqual(latest.pageInfo, {
    start: 75,
    end: 95,
    total: 95,
    hasMore: true,
    nextCursor: '75',
  })
  assert.equal(latest.contextUsage.percent, 50)
  assert.equal(latest.contextUsage.contextWindow, 128_000)

  const older = await runtime.getSessionMessagePage('session-pages', {
    limit: 20,
    before: latest.pageInfo.nextCursor,
  })
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
  const directory = await mkdtemp(join(tmpdir(), 'pisper-persisted-history-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'session.jsonl')
  const entries = [
    {
      type: 'session',
      version: 3,
      id: 'session-history',
      timestamp: '2026-01-01T00:00:00.000Z',
      cwd: directory,
    },
    {
      type: 'message',
      id: 'message-1',
      parentId: 'session-history',
      message: { role: 'user', content: 'old-user', timestamp: 1 },
    },
    {
      type: 'message',
      id: 'message-2',
      parentId: 'message-1',
      message: { role: 'assistant', content: 'old-agent', timestamp: 2 },
    },
    { type: 'compaction', id: 'compact-1', parentId: 'message-2', summary: 'compressed context' },
    {
      type: 'message',
      id: 'message-3',
      parentId: 'compact-1',
      message: { role: 'user', content: 'new-user', timestamp: 3 },
    },
    {
      type: 'message',
      id: 'message-4',
      parentId: 'message-3',
      message: { role: 'assistant', content: 'new-agent', timestamp: 4 },
    },
  ]
  await writeFile(path, `${entries.map((entry) => JSON.stringify(entry)).join('\n')}\n`, 'utf8')
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.findSessionInfo = async () => ({ path })

  const latest = await runtime.getSessionMessagePage('session-history', { limit: 2 })
  assert.deepEqual(
    latest.messages.map((message) => message.text),
    ['new-user', 'new-agent'],
  )
  assert.equal(latest.pageInfo.total, 4)
  assert.equal(latest.pageInfo.nextCursor, '2')

  const older = await runtime.getSessionMessagePage('session-history', {
    limit: 2,
    before: latest.pageInfo.nextCursor,
  })
  assert.deepEqual(
    older.messages.map((message) => message.text),
    ['old-user', 'old-agent'],
  )
  assert.equal(older.pageInfo.hasMore, false)
})

test('oversized session histories are parsed in chunks without remaining in the history cache', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-large-history-cache-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'session.jsonl')
  const entries = [
    {
      type: 'session',
      version: 3,
      id: 'session-large',
      timestamp: '2026-01-01T00:00:00.000Z',
      cwd: directory,
    },
    {
      type: 'message',
      id: 'message-1',
      parentId: 'session-large',
      message: { role: 'user', content: `large-${'x'.repeat(300)}`, timestamp: 1 },
    },
    {
      type: 'message',
      id: 'message-2',
      parentId: 'message-1',
      message: { role: 'assistant', content: 'done', timestamp: 2 },
    },
  ]
  await writeFile(path, `${entries.map((entry) => JSON.stringify(entry)).join('\n')}\n`, 'utf8')
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.findSessionInfo = async () => ({ path })
  runtime.sessionHistoryReadChunkBytes = 17
  runtime.maxSessionHistoryCacheSourceBytes = 128

  const page = await runtime.getSessionMessagePage('session-large', { limit: 2 })
  assert.equal(page.messages.length, 2)
  assert.match(page.messages[0].text, /^large-x+/)
  assert.equal(page.messages[1].text, 'done')
  assert.equal(runtime.sessionHistoryCache.has(path), false)
})

test('closed chat state is retained only while background work remains active', () => {
  assert.equal(shouldRetainClosedSessionState({ streaming: true }), true)
  assert.equal(shouldRetainClosedSessionState({ recovering: true }), true)
  assert.equal(
    shouldRetainClosedSessionState({ pendingRuntimeSelection: { thinkingLevel: 'high' } }),
    true,
  )
  assert.equal(shouldRetainClosedSessionState({ switchingModel: true }), true)
  assert.equal(shouldRetainClosedSessionState({ switchingThinking: true }), true)
  assert.equal(shouldRetainClosedSessionState({ agents: [{ status: 'running' }] }), true)
  assert.equal(shouldRetainClosedSessionState({ agents: [{ status: 'completed' }] }), false)
  assert.equal(
    shouldRetainClosedSessionState({ streaming: false, recovering: false, agents: [] }),
    false,
  )
})

test('session listings do not reopen every inactive history just to resolve its model', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-list-memory-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.listStoredSessions = async () => [
    {
      id: 'session-cached',
      path: join(directory, 'session.jsonl'),
      name: 'Cached session',
      firstMessage: 'hello',
      messageCount: 2,
      cwd: directory,
      created: new Date('2026-01-01T00:00:00.000Z'),
      modified: new Date('2026-01-01T00:01:00.000Z'),
    },
  ]
  runtime.settingsManager = {
    getGlobalSettings: () => ({ defaultProvider: 'openai', defaultModel: 'gpt-5.4' }),
  }
  runtime.goals = { get: () => null }
  runtime.plans = { get: () => null }
  runtime.multiAgents = { summaries: () => [] }
  runtime.openStoredSession = () => {
    throw new Error('inactive history should not be reparsed')
  }

  const sessions = await runtime.listSessions()
  assert.equal(sessions[0].model, 'openai/gpt-5.4')
})

test('today usage scans only newly appended session bytes', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-usage-scan-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'session.jsonl')
  const now = Date.now()
  const entries = [
    {
      type: 'session',
      version: 3,
      id: 'session-usage',
      timestamp: new Date(now - 1000).toISOString(),
      cwd: directory,
    },
    {
      type: 'message',
      id: 'assistant-1',
      parentId: null,
      timestamp: new Date(now).toISOString(),
      message: {
        role: 'assistant',
        timestamp: now,
        content: [{ type: 'text', text: 'one' }],
        usage: { input: 10, output: 5, totalTokens: 15 },
      },
    },
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

  const secondEntry = {
    type: 'message',
    id: 'assistant-2',
    parentId: 'assistant-1',
    timestamp: new Date(now + 1).toISOString(),
    message: {
      role: 'assistant',
      timestamp: now + 1,
      content: [{ type: 'text', text: 'two' }],
      usage: { input: 20, output: 10, totalTokens: 30 },
    },
  }
  await appendFile(path, `${JSON.stringify(secondEntry)}\n`, 'utf8')
  const second = await runtime.getTodayUsage()
  assert.equal(second.totalTokens, 45)
  assert.ok(runtime.usageLedger.sessionScans['session-usage'].size > firstOffset)
})

test('empty active sessions tolerate a JSONL file that has not been created yet', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-empty-session-'))
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

// 第 2 期看板卡片：由同一份运行事件增量维护，/live 带 runId，结局（完成 / 失败 / 停止）落盘为精简摘要。
function scriptedBoardSession(id, script) {
  const listeners = new Set()
  const session = {
    sessionId: id,
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier', timestamp: 1 }],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    abort: async () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async prompt() {
      session.isStreaming = true
      try {
        await script((event) => {
          for (const listener of listeners) listener(event)
        }, session)
      } finally {
        session.isStreaming = false
      }
    },
  }
  return session
}

async function boardRuntime(t, prefix) {
  const directory = await mkdtemp(join(tmpdir(), prefix))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = { getGlobalSettings: () => ({}) }
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  t.after(() => runtime.dispose().catch(() => {}))
  const install = (session) => {
    const value = { session, cwd: directory, name: 'Board card', baseToolNames: [] }
    runtime.sessions.set(session.sessionId, value)
    runtime.getOrCreateSession = async () => value
    return value
  }
  const boardItem = async (id) =>
    (await runtime.listBoardSessions(parseBoardViewQuery(new URLSearchParams()))).sessions.find(
      (session) => session.id === id,
    )
  return { directory, runtime, install, boardItem }
}

test('board cards follow run events, expose runId on /live and persist a compact summary', async (t) => {
  const { runtime, install, boardItem } = await boardRuntime(t, 'pisper-board-card-')
  let during = null
  let liveDuring = null
  const session = scriptedBoardSession('session-board-card', async (emit, current) => {
    emit({ type: 'agent_start' })
    emit({ type: 'turn_start' })
    emit({
      type: 'tool_execution_start',
      toolCallId: 'tool-1',
      toolName: 'bash',
      args: { command: 'npm test -- --reporter=dot', apiKey: 'local-value' },
    })
    emit({
      type: 'tool_execution_update',
      toolCallId: 'tool-1',
      toolName: 'bash',
      partialResult: {
        content: [
          { type: 'text', text: 'start\n\u001b[31mFAIL\u001b[0m a.test.ts\r\nrunning 3/4' },
        ],
      },
    })
    during = await boardItem(current.sessionId)
    liveDuring = await runtime.getSessionLive(current.sessionId)
    emit({
      type: 'tool_execution_end',
      toolCallId: 'tool-1',
      toolName: 'bash',
      result: { content: [{ type: 'text', text: 'done' }] },
      isError: false,
    })
    const assistant = {
      role: 'assistant',
      content: [{ type: 'text', text: '\n全部测试通过。\n细节在下面' }],
      stopReason: 'stop',
      timestamp: 2,
    }
    current.messages.push(assistant)
    emit({
      type: 'message_update',
      assistantMessageEvent: { type: 'text_delta', contentIndex: 0, delta: '全部测试通过。' },
    })
    emit({ type: 'message_end', message: assistant })
    emit({ type: 'agent_end', messages: [assistant], willRetry: false })
  })
  install(session)

  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'Run the tests.',
    runId: 'run_boardCard01',
    send: () => {},
  })

  assert.equal(liveDuring.runId, 'run_boardCard01')
  assert.equal(during.streaming, true)
  assert.equal(during.card.boardStatus, 'running')
  assert.equal(during.card.runId, 'run_boardCard01')
  assert.deepEqual(during.card.activity, {
    type: 'tool',
    name: 'bash',
    argsSummary: 'npm test -- --reporter=dot',
    startedAt: during.card.activity.startedAt,
  })
  assert.deepEqual(during.card.tail, ['start', 'FAIL a.test.ts', 'running 3/4'])
  assert.ok(during.card.startedAt)

  const after = await boardItem(session.sessionId)
  assert.equal(after.streaming, false)
  assert.equal(after.card.boardStatus, 'done')
  assert.equal(after.card.outcome, 'completed')
  assert.equal(after.card.lastAssistantPreview, '全部测试通过。')
  assert.ok(after.card.finishedAt)
  assert.equal(Object.hasOwn(after.card, 'activity'), false)
  assert.equal(Object.hasOwn(after.card, 'tail'), false)
  assert.equal(Object.hasOwn(after.card, 'error'), false)
  assert.equal((await runtime.getSessionLive(session.sessionId)).runId, 'run_boardCard01')

  const persisted = runtime.sessionMeta[session.sessionId].boardCard
  assert.equal(persisted.outcome, 'completed')
  assert.equal(persisted.lastAssistantPreview, '全部测试通过。')
  assert.equal(Object.hasOwn(persisted, 'tail'), false)

  // 模拟 Runtime 重启：内存摘要与 live 状态都没了，卡片仍由落盘摘要给出，且不再声称可挂接的 runId。
  runtime.sessionCards.forget(session.sessionId)
  runtime.liveSessions.delete(session.sessionId)
  const restored = await boardItem(session.sessionId)
  assert.equal(restored.card.outcome, 'completed')
  assert.equal(restored.card.lastAssistantPreview, '全部测试通过。')
  assert.equal(Object.hasOwn(restored.card, 'runId'), false)
  assert.equal((await runtime.getSessionLive(session.sessionId)).runId, null)
})

test('an explicit stop is reported as stopped instead of completed', async (t) => {
  const { runtime, install, boardItem } = await boardRuntime(t, 'pisper-board-stop-')
  const session = scriptedBoardSession('session-board-stop', async (emit, current) => {
    emit({ type: 'agent_start' })
    await runtime.abortSession(current.sessionId)
    current.messages.push({
      role: 'assistant',
      content: [{ type: 'text', text: '写到一半' }],
      stopReason: 'aborted',
      timestamp: 2,
    })
  })
  install(session)
  await runtime.streamPrompt({ sessionId: session.sessionId, message: 'Go.', send: () => {} })

  const item = await boardItem(session.sessionId)
  assert.equal(item.card.outcome, 'stopped')
  assert.equal(item.card.boardStatus, 'done')
  assert.equal(Object.hasOwn(item.card, 'error'), false)
  assert.equal(deriveBoardStatus(item, { now: Date.now() }).status, 'stopped')
  assert.equal(runtime.sessionMeta[session.sessionId].boardCard.outcome, 'stopped')
})

test('a failed run needs attention until dismissAttention clears it, then stays failed but handled', async (t) => {
  const { runtime, install, boardItem } = await boardRuntime(t, 'pisper-board-failure-')
  const session = scriptedBoardSession('session-board-failure', async (emit, current) => {
    emit({ type: 'agent_start' })
    emit({
      type: 'tool_execution_start',
      toolCallId: 'tool-1',
      toolName: 'bash',
      args: { command: 'npm test' },
    })
    emit({
      type: 'tool_execution_end',
      toolCallId: 'tool-1',
      toolName: 'bash',
      result: { content: [{ type: 'text', text: '3 failed, 41 passed\nexit 1' }] },
      isError: true,
    })
    current.messages.push({
      role: 'assistant',
      content: [],
      stopReason: 'error',
      errorMessage: `429 Too Many Requests ${'限'.repeat(400)}`,
      timestamp: 2,
    })
  })
  install(session)
  await runtime.streamPrompt({ sessionId: session.sessionId, message: 'Go.', send: () => {} })

  const failed = await boardItem(session.sessionId)
  assert.equal(failed.attentionReason, 'failure')
  assert.equal(failed.card.boardStatus, 'attention')
  assert.equal(failed.card.outcome, 'failed')
  assert.equal(failed.card.error.kind, 'model')
  assert.equal([...failed.card.error.message].length, 240)
  assert.deepEqual(failed.card.tail, ['3 failed, 41 passed', 'exit 1'])

  const organizationRoute = sessionRuntimeRoutes.find(
    (route) => route.method === 'PATCH' && route.path === '/api/sessions/:sessionId/organization',
  )
  const patch = async (input) => {
    let response
    await organizationRoute.handler({
      runtime,
      params: { sessionId: session.sessionId },
      body: async () => input,
      json: (status, value) => {
        response = { status, value }
      },
    })
    return response
  }
  const rejected = await patch({ dismissAttention: false })
  assert.equal(rejected.status, 400)
  assert.equal(rejected.value.code, 'invalid_session_organization')

  const dismissed = await patch({ dismissAttention: true })
  assert.equal(dismissed.status, 200)
  assert.equal(dismissed.value.needsAttention, false)
  assert.equal(dismissed.value.attentionReason, null)
  // 只清注意力，不改已读状态。
  assert.equal(dismissed.value.unread, true)

  const handled = await boardItem(session.sessionId)
  assert.equal(handled.card.boardStatus, 'done')
  assert.equal(handled.card.outcome, 'failed')
  assert.deepEqual(deriveBoardStatus(handled, { now: Date.now() }), {
    lane: 'done',
    status: 'failed',
    handled: true,
    since: Date.parse(handled.lastCompletedAt),
  })
})

test('the board event stream ends a run with a not-running frame even when finishing writes are slow', async (t) => {
  const { runtime, install } = await boardRuntime(t, 'pisper-board-slow-finish-')
  const session = scriptedBoardSession('session-board-slow-finish', async (emit, current) => {
    emit({ type: 'agent_start' })
    emit({ type: 'turn_start' })
    const assistant = {
      role: 'assistant',
      content: [{ type: 'text', text: 'ok' }],
      stopReason: 'stop',
      timestamp: 2,
    }
    current.messages.push(assistant)
    emit({ type: 'message_end', message: assistant })
    emit({ type: 'agent_end', messages: [assistant], willRetry: false })
  })
  install(session)

  const frames = []
  const connection = runtime.boardEvents.connect({
    after: null,
    send: (event, data) => {
      if (event === 'session_card' && data?.id === session.sessionId) frames.push(data)
    },
    close: () => {},
  })
  t.after(() => connection.detach())
  await connection.ready

  // 慢磁盘：运行收尾的元数据落盘晚于看板 flush（移动端、杀软扫描下常见）。
  const save = runtime.saveSessionMeta.bind(runtime)
  runtime.saveSessionMeta = async () => {
    await new Promise((resolve) => setTimeout(resolve, 350))
    return save()
  }
  t.after(() => {
    runtime.saveSessionMeta = save
  })
  await runtime.streamPrompt({ sessionId: session.sessionId, message: 'Go.', send: () => {} })

  const deadline = Date.now() + 2_000
  while (Date.now() < deadline && frames.at(-1)?.session?.streaming !== false) {
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
  const last = frames.at(-1)
  assert.ok(last, 'the run produced at least one card frame')
  assert.equal(last.session.streaming, false)
  assert.equal(last.card.boardStatus, 'done')
  assert.equal(last.card.outcome, 'completed')
})
