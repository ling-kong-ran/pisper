import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { DEFAULT_MAX_BYTES } from '@earendil-works/pi-coding-agent'
import { MULTI_AGENT_TOOL_NAMES, MultiAgentService } from '../services/multi-agent-service.mjs'
import {
  TOOL_CATALOG,
  TOOL_PRESETS,
  createMultiAgentTools,
  toolsFromConfig,
} from '../tools/registry.mjs'

function deferred() {
  let resolve
  let reject
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

async function waitFor(predicate, message = 'condition') {
  const deadline = Date.now() + 2_000
  while (Date.now() < deadline) {
    const value = predicate()
    if (value) return value
    await new Promise((resolve) => setTimeout(resolve, 0))
  }
  throw new Error(`Timed out waiting for ${message}.`)
}

function createFakeSession({ onPrompt } = {}) {
  const listeners = new Set()
  const session = {
    agent: { state: { systemPrompt: 'Base system prompt' } },
    messages: [],
    promptCalls: [],
    steerCalls: [],
    followUpCalls: [],
    aborted: false,
    disposed: false,
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    emit(event) {
      for (const listener of listeners) listener(event)
    },
    async prompt(message) {
      session.promptCalls.push(message)
      await onPrompt?.({ message, session })
    },
    async steer(message) {
      session.steerCalls.push(message)
    },
    async followUp(message) {
      session.followUpCalls.push(message)
    },
    async abort() {
      session.aborted = true
    },
    dispose() {
      session.disposed = true
    },
  }
  return session
}

function createService(session, overrides = {}) {
  const seen = { options: null, messages: [] }
  const service = new MultiAgentService({
    getModelRuntime: () => ({ id: 'runtime' }),
    getSettingsManager: () => ({ id: 'settings' }),
    createResourceLoader: async (options) => ({ options }),
    createSessionManager: () => ({
      appendMessage(message) {
        seen.messages.push(message)
      },
    }),
    createSession: async (options) => {
      seen.options = options
      return { session }
    },
    ...overrides,
  })
  return { service, seen }
}

function baseInput(overrides = {}) {
  return {
    parentSessionId: 'parent-1',
    cwd: process.cwd(),
    model: { provider: 'openai', id: 'gpt-5', contextWindow: 200_000, maxTokens: 128_000 },
    thinkingLevel: 'high',
    taskName: 'inspect_runtime',
    message: 'Inspect the runtime and report the relevant files.',
    allowedTools: [
      'read',
      'edit',
      'bash',
      'spawn_agent',
      'wait_agent',
      'get_goal',
      'browser_automation',
      'mcp_manage',
    ],
    customTools: [{ name: 'bash' }, { name: 'spawn_agent' }, { name: 'browser_automation' }],
    ...overrides,
  }
}

test('child custom tool factory receives the child identity and disposes its resources', async () => {
  const session = createFakeSession({
    onPrompt: async ({ session: active }) => {
      active.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'done' }],
        usage: { input: 1, output: 1, totalTokens: 2 },
      })
    },
  })
  const contexts = []
  let disposed = 0
  const { service, seen } = createService(session, { terminalSessionRetentionMs: 0 })
  const started = await service.spawn(
    baseInput({
      customTools: undefined,
      createCustomTools: async (context) => {
        contexts.push(context)
        return { tools: [{ name: 'bash' }], dispose: async () => (disposed += 1) }
      },
    }),
  )
  await waitFor(() => service.list('parent-1')[0]?.status === 'completed', 'Agent completion')
  await waitFor(() => disposed === 1, 'custom tool disposal')
  assert.equal(contexts[0].id, started.id)
  assert.equal(contexts[0].parentSessionId, 'parent-1')
  assert.deepEqual(seen.options.customTools, [{ name: 'bash' }])
  await service.dispose()
})

test('spawn_agent starts asynchronously and inherits the active model, reasoning level, and safe tools', async () => {
  const promptGate = deferred()
  const session = createFakeSession({
    onPrompt: async ({ session: active }) => {
      await promptGate.promise
      active.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Inspected the runtime.' }],
        usage: { input: 20, output: 8, totalTokens: 28 },
      })
    },
  })
  const { service, seen } = createService(session)

  const started = await service.spawn(baseInput())
  assert.ok(['starting', 'running'].includes(started.status))
  assert.equal(Object.hasOwn(started, 'role'), false)
  assert.equal(Object.hasOwn(started, 'dependsOn'), false)
  assert.equal(started.maxTurns, 30)
  await waitFor(() => session.promptCalls.length === 1, 'Agent prompt start')
  assert.equal(service.list('parent-1')[0].status, 'running')
  assert.equal(service.list('parent-1')[0].currentActivity.type, 'model')
  session.emit({
    type: 'tool_execution_start',
    toolCallId: 'read-live',
    toolName: 'read',
    args: { path: 'runtime/runtime.mjs' },
  })
  assert.deepEqual(service.list('parent-1')[0].currentActivity.args, {
    path: 'runtime/runtime.mjs',
  })
  session.emit({
    type: 'tool_execution_end',
    toolCallId: 'read-live',
    toolName: 'read',
    isError: false,
  })
  assert.equal(service.list('parent-1')[0].currentActivity.stage, 'processing_result')
  assert.equal(seen.options.model.id, 'gpt-5')
  assert.equal(seen.options.thinkingLevel, 'high')
  assert.deepEqual(seen.options.tools, ['read', 'edit', 'bash'])
  assert.deepEqual(seen.options.customTools, [{ name: 'bash' }])
  assert.ok(MULTI_AGENT_TOOL_NAMES.every((name) => seen.options.excludeTools.includes(name)))
  assert.ok(seen.options.excludeTools.includes('get_goal'))
  assert.ok(seen.options.excludeTools.includes('browser_automation'))

  promptGate.resolve()
  const completed = await waitFor(
    () => service.list('parent-1')[0]?.status === 'completed' && service.list('parent-1')[0],
    'Agent completion',
  )
  assert.equal(completed.output, 'Inspected the runtime.')
  assert.deepEqual(completed.runUsage, {
    input: 20,
    output: 8,
    cacheRead: 0,
    cacheWrite: 0,
    reasoning: 0,
    totalTokens: 28,
  })
})

test('subagents inherit parent-safe tools without hard-coded roles', async () => {
  const session = createFakeSession({
    onPrompt: async ({ session: active }) =>
      active.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Task complete.' }],
      }),
  })
  const { service, seen } = createService(session)
  await service.spawn(
    baseInput({ allowedTools: ['read', 'grep', 'find', 'edit', 'write', 'bash', 'spawn_agent'] }),
  )
  await waitFor(
    () => service.list('parent-1')[0]?.status === 'completed',
    'generic subagent completion',
  )
  assert.deepEqual(seen.options.tools, ['read', 'grep', 'find', 'edit', 'write', 'bash'])
  assert.match(seen.options.resourceLoader.options.appendSystemPrompt, /isolated context/)
  assert.doesNotMatch(seen.options.resourceLoader.options.appendSystemPrompt, /Role:/)
})

test('subagents inherit canonical read-only plan access while update names remain parent-only', async () => {
  const session = createFakeSession({
    onPrompt: async ({ session: active }) =>
      active.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Read the shared plan.' }],
      }),
  })
  const getPlan = { name: 'get_plan' }
  const updatePlan = { name: 'update_plan' }
  const legacyUpdatePlan = { name: 'update_task_list' }
  const { service, seen } = createService(session)

  await service.spawn(
    baseInput({
      allowedTools: ['read', 'get_plan', 'update_plan', 'update_task_list'],
      customTools: [getPlan, updatePlan, legacyUpdatePlan],
    }),
  )
  await waitFor(() => service.list('parent-1')[0]?.status === 'completed', 'shared plan read')

  assert.deepEqual(seen.options.tools, ['read', 'get_plan'])
  assert.deepEqual(seen.options.customTools, [getPlan])
  assert.ok(seen.options.excludeTools.includes('update_plan'))
  assert.ok(seen.options.excludeTools.includes('update_task_list'))
})

test('send_message steers a running Agent and followup_task reuses its context', async () => {
  const firstRun = deferred()
  const session = createFakeSession({
    onPrompt: async ({ message, session: active }) => {
      if (message.includes('Inspect')) await firstRun.promise
      active.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: `Completed: ${message}` }],
        usage: { input: 5, output: 5, totalTokens: 10 },
      })
    },
  })
  const { service } = createService(session)
  const started = await service.spawn(baseInput())
  await waitFor(() => service.list('parent-1')[0]?.status === 'running', 'running Agent')

  await service.sendMessage('parent-1', started.id, 'Focus on the service boundary.')
  await service.followup('parent-1', started.id, 'Also check the tests.')
  assert.deepEqual(session.steerCalls, ['Focus on the service boundary.'])
  assert.deepEqual(session.followUpCalls, ['Also check the tests.'])

  firstRun.resolve()
  await waitFor(() => service.list('parent-1')[0]?.status === 'completed', 'first Agent run')
  const [firstDelivery] = service.peekMailbox('parent-1')
  assert.equal(firstDelivery.resultVersion, 1)
  await assert.rejects(service.sendMessage('parent-1', started.id, 'Late message'), /not running/)

  const queued = await service.followup('parent-1', started.id, 'Review the completed result.')
  assert.ok(['queued', 'starting', 'running', 'completed'].includes(queued.status))
  await waitFor(() => session.promptCalls.length === 2, 'follow-up prompt')
  const completed = await waitFor(
    () =>
      service.list('parent-1')[0]?.status === 'completed' &&
      service.list('parent-1')[0]?.runNumber === 2 &&
      service.list('parent-1')[0],
    'follow-up completion',
  )
  assert.equal(completed.runNumber, 2)
  assert.equal(completed.runUsage.totalTokens, 10)
  const deliveries = service.peekMailbox('parent-1')
  assert.deepEqual(
    deliveries.map((agent) => agent.resultVersion),
    [1, 2],
  )
  assert.match(deliveries[0].output, /Inspect the runtime/)
  assert.match(deliveries[1].output, /Review the completed result/)
  const pending = await service.wait('parent-1', 250)
  assert.equal(pending.agent.mailboxId, firstDelivery.mailboxId)
  await service.acknowledge('parent-1', [firstDelivery])
  assert.deepEqual(
    service.peekMailbox('parent-1').map((agent) => agent.resultVersion),
    [2],
  )
})

test('Agents have no wall-clock timeout while Agent turn limits ignore ordinary tool calls', async () => {
  const timers = new Map()
  const setTimer = (callback, milliseconds) => {
    const handle = { milliseconds, unref() {} }
    timers.set(handle, callback)
    return handle
  }
  const clearTimer = (handle) => timers.delete(handle)
  const promptGate = deferred()
  const session = createFakeSession({ onPrompt: () => promptGate.promise })
  const { service } = createService(session, { setTimer, clearTimer })
  const started = await service.spawn(baseInput())
  await waitFor(() => service.list('parent-1')[0]?.status === 'running', 'unbounded Agent')
  assert.equal(timers.size, 0)
  assert.equal(session.aborted, false)
  service.interrupt('parent-1', started.id, 'explicit stop')
  await waitFor(() => session.aborted, 'explicit Agent interruption')
  promptGate.resolve()
  const interrupted = await waitFor(
    () => service.list('parent-1')[0]?.status === 'interrupted' && service.list('parent-1')[0],
    'explicit interruption',
  )
  assert.equal(interrupted.error, 'explicit stop')

  const secondGate = deferred()
  const secondSession = createFakeSession({ onPrompt: () => secondGate.promise })
  const { service: secondService } = createService(secondSession)
  const second = await secondService.spawn(baseInput({ parentSessionId: 'parent-2', maxTurns: 1 }))
  await waitFor(() => secondService.list('parent-2')[0]?.status === 'running', 'turn-limited Agent')
  secondSession.emit({ type: 'turn_start' })
  for (let index = 0; index < 40; index += 1) {
    secondSession.emit({
      type: 'tool_execution_start',
      toolCallId: `read-${index}`,
      toolName: 'read',
    })
    secondSession.emit({
      type: 'tool_execution_end',
      toolCallId: `read-${index}`,
      toolName: 'read',
      isError: false,
    })
  }
  assert.equal(secondSession.aborted, false)
  assert.equal(secondService.list('parent-2')[0].toolCallCount, 40)
  secondSession.emit({ type: 'turn_start' })
  await waitFor(() => secondSession.aborted, 'turn limit abort')
  secondGate.resolve()
  const turnInterrupted = await waitFor(
    () =>
      secondService.list('parent-2')[0]?.status === 'interrupted' &&
      secondService.list('parent-2')[0],
    'turn limit interruption',
  )
  assert.equal(turnInterrupted.id, second.id)
  assert.match(turnInterrupted.error, /1-turn limit/)
})

test('wait_agent ignores progress noise, returns on terminal state, and abortParent stays scoped', async () => {
  const gate = deferred()
  const session = createFakeSession({
    onPrompt: async ({ session: active }) => {
      await gate.promise
      active.messages.push({ role: 'assistant', content: [{ type: 'text', text: 'Done.' }] })
    },
  })
  const { service } = createService(session)
  const started = await service.spawn(baseInput())
  await waitFor(() => service.list('parent-1')[0]?.status === 'running', 'waiting Agent')

  let settled = false
  const waiting = service.wait('parent-1', 30_000, started.id).then((value) => {
    settled = true
    return value
  })
  session.emit({ type: 'tool_execution_start', toolCallId: 'read-1', toolName: 'read' })
  await Promise.resolve()
  assert.equal(settled, false)
  assert.equal(service.abortParent('another-parent'), 0)
  gate.resolve()
  const update = await waiting
  assert.equal(update.timedOut, false)
  assert.equal(update.agent.id, started.id)
  assert.equal(update.agent.status, 'completed')
  assert.equal(update.agents[0].status, 'completed')
  assert.equal(update.agents[0].toolCallCount, 1)
  assert.deepEqual(
    service.peekMailbox('parent-1').map((agent) => agent.id),
    [started.id],
  )
  await service.acknowledge('parent-1', [update.agent])
  assert.deepEqual(service.peekMailbox('parent-1'), [])

  const secondGate = deferred()
  const secondSession = createFakeSession({ onPrompt: () => secondGate.promise })
  const { service: secondService } = createService(secondSession)
  await secondService.spawn(baseInput())
  await waitFor(() => secondService.list('parent-1')[0]?.status === 'running', 'abortable Agent')
  assert.equal(secondService.abortParent('another-parent'), 0)
  assert.equal(secondService.abortParent('parent-1'), 1)
  secondGate.resolve()
  await waitFor(() => secondService.list('parent-1')[0]?.status === 'interrupted', 'parent abort')
})

test('completed, failed, and interrupted Agents emit terminal callbacks without polling', async () => {
  const terminal = []

  const completedSession = createFakeSession({
    onPrompt: async ({ session }) =>
      session.messages.push({ role: 'assistant', content: [{ type: 'text', text: 'done' }] }),
  })
  const { service: completedService } = createService(completedSession)
  await completedService.spawn(
    baseInput({ taskName: 'complete', onTerminal: (agent) => terminal.push(agent) }),
  )
  await waitFor(
    () => terminal.some((agent) => agent.status === 'completed'),
    'completed terminal callback',
  )

  const failedSession = createFakeSession({
    onPrompt: async () => {
      throw new Error('child failed')
    },
  })
  const { service: failedService } = createService(failedSession)
  await failedService.spawn(
    baseInput({ taskName: 'fail', onTerminal: (agent) => terminal.push(agent) }),
  )
  await waitFor(
    () => terminal.some((agent) => agent.status === 'failed'),
    'failed terminal callback',
  )

  const gate = deferred()
  const interruptedSession = createFakeSession({ onPrompt: async () => gate.promise })
  const { service: interruptedService } = createService(interruptedSession)
  const started = await interruptedService.spawn(
    baseInput({ taskName: 'interrupt', onTerminal: (agent) => terminal.push(agent) }),
  )
  await waitFor(() => interruptedService.list('parent-1')[0]?.status === 'running', 'running Agent')
  interruptedService.interrupt('parent-1', started.id)
  await waitFor(
    () => terminal.some((agent) => agent.status === 'interrupted'),
    'interrupted terminal callback',
  )
  gate.resolve()
})

test('Agent output is UTF-8 safe and bounded to the Pisper tool-output limit', async () => {
  const largeOutput = '你'.repeat(20_000)
  const session = createFakeSession({
    onPrompt: async ({ session: active }) =>
      active.messages.push({ role: 'assistant', content: [{ type: 'text', text: largeOutput }] }),
  })
  const { service } = createService(session)
  await service.spawn(baseInput())
  const completed = await waitFor(
    () => service.list('parent-1')[0]?.status === 'completed' && service.list('parent-1')[0],
    'bounded output',
  )
  assert.ok(Buffer.byteLength(completed.output, 'utf8') <= DEFAULT_MAX_BYTES)
  assert.equal(completed.output.includes('\ufffd'), false)
  assert.equal(completed.outputTruncated, true)
  assert.match(completed.output, /Pisper tool-output limit/)
  assert.doesNotMatch(completed.output, /Pi tool-output limit/)
  assert.equal(completed.fullOutput, completed.output)
})

test('terminal Agent sessions and callbacks are released after the follow-up retention window', async () => {
  let releaseTimer = null
  const setTimer = (callback, milliseconds) => {
    const timer = { callback, milliseconds, unref() {} }
    releaseTimer = timer
    return timer
  }
  const clearTimer = (timer) => {
    if (releaseTimer === timer) releaseTimer = null
  }
  const session = createFakeSession({
    onPrompt: async ({ session: active }) =>
      active.messages.push({ role: 'assistant', content: [{ type: 'text', text: 'Done.' }] }),
  })
  const { service } = createService(session, {
    terminalSessionRetentionMs: 500,
    setTimer,
    clearTimer,
  })
  const started = await service.spawn(baseInput())
  await waitFor(
    () => releaseTimer && service.list('parent-1')[0]?.status === 'completed',
    'Agent retention timer',
  )
  assert.equal(releaseTimer.milliseconds, 500)
  assert.equal(session.disposed, false)

  releaseTimer.callback()
  const record = service.records.get(started.id)
  assert.equal(session.disposed, true)
  assert.equal(record.session, null)
  assert.deepEqual(record.customTools, [])
  assert.equal(record.onProgress, null)
  await assert.rejects(service.followup('parent-1', started.id, 'Continue.'), /context expired/)
})

test('Codex-style Agent tools replace delegate_task and stay hidden from the plugins catalog', async () => {
  assert.equal(
    TOOL_CATALOG.some((tool) => tool.id === 'delegate_task'),
    false,
  )
  assert.ok(MULTI_AGENT_TOOL_NAMES.every((name) => !TOOL_CATALOG.some((tool) => tool.id === name)))
  let input
  const [tool] = createMultiAgentTools({
    multiAgentRuntime: {
      spawn: async (value) => {
        input = value
        return { id: 'agent-1', canonicalName: '/root/inspect_1', status: 'starting' }
      },
    },
  })
  const result = await tool.execute('spawn-1', {
    taskName: 'inspect',
    message: 'Inspect the runtime.',
    maxTurns: 30,
  })
  assert.deepEqual(input, { taskName: 'inspect', message: 'Inspect the runtime.', maxTurns: 30 })
  assert.match(result.content[0].text, /Started \/root\/inspect_1 in the background/)
  assert.equal(Object.hasOwn(tool.parameters.properties, 'role'), false)
  assert.equal(Object.hasOwn(tool.parameters.properties, 'dependsOn'), false)
  assert.equal(Object.hasOwn(tool.parameters.properties, 'maxToolCalls'), false)
  assert.equal(Object.hasOwn(tool.parameters.properties, 'maxDurationSeconds'), false)
  assert.ok(Object.hasOwn(tool.parameters.properties, 'maxTurns'))
})

test('list_agents returns statuses without a task graph or role expansion', async () => {
  const tools = createMultiAgentTools({
    multiAgentRuntime: {
      list: async () => [
        {
          id: 'agent-1',
          canonicalName: '/root/inspect_1',
          status: 'queued',
          output: '',
          error: '',
        },
      ],
    },
  })
  const list = tools.find((tool) => tool.name === 'list_agents')
  const result = await list.execute('list-1', {})
  assert.match(result.content[0].text, /\/root\/inspect_1 · queued/)
  assert.equal(Object.hasOwn(result.details, 'graph'), false)
})

test('background Agents do not instruct the parent to poll or wait by default', () => {
  const tools = createMultiAgentTools({ multiAgentRuntime: {} })
  const spawn = tools.find((tool) => tool.name === 'spawn_agent')
  const list = tools.find((tool) => tool.name === 'list_agents')
  const wait = tools.find((tool) => tool.name === 'wait_agent')
  const spawnGuidance = spawn.promptGuidelines.join('\n')
  const listGuidance = list.promptGuidelines.join('\n')
  const waitGuidance = wait.promptGuidelines.join('\n')

  assert.match(spawnGuidance, /must not delay replying to the user/)
  assert.match(spawnGuidance, /do not call list_agents or wait_agent merely to monitor progress/i)
  assert.match(spawnGuidance, /never injected into the parent prompt or model context/)
  assert.match(spawnGuidance, /spawning other independent Agents/)
  assert.doesNotMatch(spawnGuidance, /explorer|reviewer|dependsOn/)
  assert.match(listGuidance, /Do not call list_agents repeatedly/)
  assert.match(waitGuidance, /Never call wait_agent repeatedly after a timeout/)
  assert.match(waitGuidance, /reply while Agents continue in the background/)
  assert.match(waitGuidance, /without injecting model context/)
})

test('new installations enable the complete tool catalog by default', () => {
  assert.deepEqual(toolsFromConfig({}), TOOL_PRESETS.full)
  assert.deepEqual(new Set(TOOL_PRESETS.full), new Set(TOOL_CATALOG.map((tool) => tool.id)))
  assert.ok(MULTI_AGENT_TOOL_NAMES.every((name) => !TOOL_PRESETS.full.includes(name)))
})

test('interrupt then followup waits for the old run and keeps run generations isolated', async () => {
  const firstPrompt = deferred()
  const secondPrompt = deferred()
  let promptCount = 0
  const session = createFakeSession({
    onPrompt: async ({ message, session: active }) => {
      promptCount += 1
      if (promptCount === 1) {
        await firstPrompt.promise
        active.messages.push({
          role: 'assistant',
          content: [{ type: 'text', text: `stale:${message}` }],
        })
        return
      }
      await secondPrompt.promise
      active.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: `fresh:${message}` }],
      })
    },
  })
  const { service } = createService(session)
  const started = await service.spawn(baseInput())
  await waitFor(() => service.list('parent-1')[0]?.status === 'running', 'first run')

  service.interrupt('parent-1', started.id, 'stop first run')
  const followup = service.followup('parent-1', started.id, 'Continue with the fixed plan.')
  // Old prompt still finishing; followup must not start the next run until it settles.
  assert.equal(promptCount, 1)
  firstPrompt.resolve()
  await followup
  await waitFor(
    () => promptCount === 2 && service.list('parent-1')[0]?.status === 'running',
    'follow-up run',
  )

  secondPrompt.resolve()
  const completed = await waitFor(
    () =>
      service.list('parent-1')[0]?.status === 'completed' &&
      service.list('parent-1')[0]?.runNumber === 2 &&
      service.list('parent-1')[0],
    'safe follow-up completion',
  )
  assert.equal(completed.runNumber, 2)
  assert.match(completed.output, /fresh:Continue with the fixed plan/)
  assert.equal(completed.output.includes('stale:'), false)
})

test('Agent registry survives restart and marks previously active runs as interrupted', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-agents-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'agents.json')
  await writeFile(
    path,
    `${JSON.stringify(
      {
        version: 4,
        sequence: 7,
        records: [
          {
            id: 'agent-running',
            taskName: 'review_runtime',
            canonicalName: '/root/review_runtime_7',
            parentSessionId: 'parent-persisted',
            cwd: directory,
            model: 'openai/gpt-5',
            thinkingLevel: 'high',
            message: 'Review the runtime.',
            availableTools: ['read'],
            maxTurns: 24,
            turnCount: 3,
            toolCallCount: 1,
            tools: [{ id: 'read-1', name: 'read', status: 'running' }],
            output: '',
            outputTruncated: false,
            usage: {},
            runUsage: {},
            runNumber: 1,
            resultVersion: 0,
            error: '',
            startedAt: '2026-07-23T10:00:00.000Z',
            lastActivityAt: '2026-07-23T10:00:05.000Z',
            completedAt: null,
            durationMs: null,
            status: 'running',
          },
        ],
      },
      null,
      2,
    )}\n`,
    'utf8',
  )
  const service = new MultiAgentService({
    path,
    now: () => new Date('2026-07-23T10:01:00.000Z').getTime(),
  })
  await service.init()

  const [restored] = service.list('parent-persisted')
  assert.equal(restored.status, 'interrupted')
  assert.equal(restored.maxTurns, 24)
  assert.equal(restored.turnCount, 3)
  assert.match(restored.error, /Pisper restarted/)
  assert.equal(restored.completedAt, '2026-07-23T10:01:00.000Z')
  assert.equal(restored.resultVersion, 1)
  assert.deepEqual(
    service.peekMailbox('parent-persisted').map((agent) => agent.id),
    ['agent-running'],
  )

  await service.acknowledge('parent-persisted', [restored])
  assert.deepEqual(service.peekMailbox('parent-persisted'), [])
  const persisted = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(persisted.records[0].status, 'interrupted')
  assert.deepEqual(persisted.mailbox, [])
})

test('legacy Agent registries are discarded instead of migrated', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-agent-legacy-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'agents.json')
  await writeFile(
    path,
    `${JSON.stringify(
      {
        version: 2,
        sequence: 9,
        records: [
          {
            id: 'legacy-agent',
            parentSessionId: 'parent-legacy',
            model: 'openai/gpt-5',
            role: 'reviewer',
            dependsOn: ['another-agent'],
            maxToolCalls: 30,
            status: 'completed',
          },
        ],
      },
      null,
      2,
    )}\n`,
    'utf8',
  )

  const service = new MultiAgentService({ path })
  await service.init()

  assert.deepEqual(service.list('parent-legacy'), [])
  const persisted = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(persisted.version, 4)
  assert.equal(persisted.sequence, 0)
  assert.deepEqual(persisted.records, [])
})

test('completed Agent results persist across service instances', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-agent-result-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'agents.json')
  const session = createFakeSession({
    onPrompt: async ({ session: active }) =>
      active.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Persisted result.' }],
      }),
  })
  const { service } = createService(session, { path })
  await service.init()
  await service.spawn(baseInput({ cwd: directory, maxTurns: 12 }))
  await waitFor(
    () => service.list('parent-1')[0]?.status === 'completed',
    'persisted Agent completion',
  )
  await service.flush()

  const restored = new MultiAgentService({ path })
  await restored.init()
  const [record] = restored.list('parent-1')
  assert.equal(record.status, 'completed')
  assert.equal(record.maxTurns, 12)
  assert.equal(Object.hasOwn(record, 'role'), false)
  assert.equal(record.output, 'Persisted result.')
  assert.equal(restored.peekMailbox('parent-1').length, 1)
  await service.dispose()
  await restored.dispose()
})

test('spawned Agents always start with an isolated conversation', async () => {
  const session = createFakeSession({
    onPrompt: async ({ session: active }) =>
      active.messages.push({ role: 'assistant', content: [{ type: 'text', text: 'Done.' }] }),
  })
  const { service, seen } = createService(session)
  await service.spawn(baseInput())
  await waitFor(
    () => service.list('parent-1')[0]?.status === 'completed',
    'isolated Agent completion',
  )
  assert.deepEqual(seen.messages, [])
})

test('messages sent while an Agent is starting are delivered after prompt startup', async () => {
  const loaderGate = deferred()
  const promptGate = deferred()
  const session = createFakeSession({
    onPrompt: async ({ session: active }) => {
      await promptGate.promise
      active.messages.push({ role: 'assistant', content: [{ type: 'text', text: 'Done.' }] })
    },
  })
  const { service } = createService(session, {
    createResourceLoader: async (options) => {
      await loaderGate.promise
      return { options }
    },
  })
  const started = await service.spawn(baseInput())
  await service.sendMessage('parent-1', started.id, 'Prioritize the runtime boundary.')
  await service.followup('parent-1', started.id, 'Then summarize the tests.')
  assert.deepEqual(session.steerCalls, [])
  assert.deepEqual(session.followUpCalls, [])

  loaderGate.resolve()
  await waitFor(
    () =>
      session.promptCalls.length === 1 &&
      session.steerCalls.length === 1 &&
      session.followUpCalls.length === 1,
    'queued startup messages',
  )
  assert.deepEqual(session.steerCalls, ['Prioritize the runtime boundary.'])
  assert.deepEqual(session.followUpCalls, ['Then summarize the tests.'])
  promptGate.resolve()
  await waitFor(
    () => service.list('parent-1')[0]?.status === 'completed',
    'queued-message Agent completion',
  )
})

test('global concurrency queues Agents in FIFO order without task-graph semantics', async () => {
  const firstGate = deferred()
  const firstSession = createFakeSession({
    onPrompt: async ({ session }) => {
      await firstGate.promise
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'First complete.' }],
      })
    },
  })
  const secondSession = createFakeSession({
    onPrompt: async ({ session }) =>
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Second complete.' }],
      }),
  })
  const sessions = [firstSession, secondSession]
  const { service } = createService(firstSession, {
    maxConcurrent: 1,
    createSession: async (options) => ({ session: sessions.shift(), options }),
  })

  const first = await service.spawn(baseInput({ taskName: 'first' }))
  await waitFor(
    () => service.list('parent-1').find((agent) => agent.id === first.id)?.status === 'running',
    'first Agent',
  )
  const second = await service.spawn(baseInput({ taskName: 'second' }))
  assert.equal(second.status, 'queued')
  assert.equal(secondSession.promptCalls.length, 0)
  assert.equal(Object.hasOwn(second, 'dependsOn'), false)

  firstGate.resolve()
  await waitFor(
    () => service.list('parent-1').find((agent) => agent.id === second.id)?.status === 'completed',
    'second Agent',
  )
  assert.equal(secondSession.promptCalls.length, 1)
})

test('slow startup remains active without a wall-clock timeout and retains its concurrency slot', async () => {
  const timers = new Map()
  const setTimer = (callback, milliseconds) => {
    const handle = { milliseconds, unref() {} }
    timers.set(handle, callback)
    return handle
  }
  const clearTimer = (handle) => timers.delete(handle)
  const loaderGate = deferred()
  let loaderCount = 0
  const firstSession = createFakeSession({
    onPrompt: async ({ session }) =>
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'First completed.' }],
      }),
  })
  const secondSession = createFakeSession({
    onPrompt: async ({ session }) =>
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Second completed.' }],
      }),
  })
  const sessions = [firstSession, secondSession]
  const { service } = createService(firstSession, {
    maxConcurrent: 1,
    setTimer,
    clearTimer,
    createResourceLoader: async (options) => {
      loaderCount += 1
      if (loaderCount === 1) await loaderGate.promise
      return { options }
    },
    createSession: async () => ({ session: sessions.shift() }),
  })
  const started = await service.spawn(baseInput())
  const queued = await service.spawn(baseInput({ parentSessionId: 'parent-2', taskName: 'second' }))
  assert.equal(queued.status, 'queued')
  assert.equal(service.list('parent-1')[0].status, 'starting')
  assert.equal(service.list('parent-2')[0].status, 'queued')
  assert.equal(timers.size, 0)

  loaderGate.resolve()
  const firstCompleted = await waitFor(
    () => service.list('parent-1')[0]?.status === 'completed' && service.list('parent-1')[0],
    'first Agent completion',
  )
  const secondCompleted = await waitFor(
    () => service.list('parent-2')[0]?.status === 'completed' && service.list('parent-2')[0],
    'queued Agent completion',
  )
  assert.equal(firstCompleted.output, 'First completed.')
  assert.equal(secondCompleted.output, 'Second completed.')
  assert.equal(service.list('parent-1')[0].id, started.id)
})
