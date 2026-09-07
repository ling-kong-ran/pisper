import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { createAgentSession, SessionManager, SettingsManager } from '../runtime/pi-coding-agent.mjs'
import {
  captureQueuedSessionInput,
  sessionInputQueueRevision,
  withdrawQueuedSessionInput,
} from '../runtime/session-input-queue.mjs'
import { queuedSessionInputs } from '../runtime/stream-projection.mjs'

const { createExtensionRuntime } = await import(
  new URL('./core/extensions/loader.js', import.meta.resolve('@earendil-works/pi-coding-agent'))
    .href
)

function request(method, body) {
  return {
    method,
    async *[Symbol.asyncIterator]() {
      if (body !== undefined) yield Buffer.from(JSON.stringify(body))
    },
  }
}

function response() {
  return {
    status: 0,
    body: '',
    writeHead(status) {
      this.status = status
    },
    end(body = '') {
      this.body = body
    },
  }
}

test('running sessions accept steering and follow-up user messages through the Pi queue', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-input-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose?.().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  const calls = []
  const steering = []
  const followUp = []
  const session = {
    sessionId: 'session-1',
    isStreaming: true,
    pendingMessageCount: 2,
    getSteeringMessages: () => steering,
    getFollowUpMessages: () => followUp,
    async prompt(message, options) {
      calls.push({ message, options })
      if (options.streamingBehavior === 'followUp') followUp.push(message)
      else steering.push(message)
    },
  }
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => [null, { path: join(directory, 'diagram.png') }]
  const selections = []
  runtime.selectToolsForMessage = (_value, message, options) => {
    selections.push({ message, options })
  }
  runtime.sessions.set('session-1', { session, modified: '' })

  assert.deepEqual(
    await runtime.queueSessionMessage('session-1', {
      message: 'Focus on the Windows path.',
      attachments: [
        { kind: 'text', name: 'notes.md', text: 'Use the Win32 path.' },
        { kind: 'image', name: 'diagram.png', mimeType: 'image/png', data: 'aW1hZ2U=' },
      ],
      behavior: 'steer',
    }),
    {
      queued: true,
      behavior: 'steer',
      inputId: null,
      queueRevision: sessionInputQueueRevision(session),
      pendingMessageCount: 2,
      queuedInputs: [{ behavior: 'steer', text: 'Focus on the Windows path.' }],
    },
  )
  assert.match(calls[0].message, /^Focus on the Windows path\./)
  assert.match(calls[0].message, /\[Text attachment: notes\.md\]\nUse the Win32 path\./)
  assert.match(calls[0].message, /\[Image attachment\] diagram\.png/)
  assert.deepEqual(calls[0].options, {
    images: [{ type: 'image', data: 'aW1hZ2U=', mimeType: 'image/png' }],
    streamingBehavior: 'steer',
    source: 'interactive',
  })
  assert.deepEqual(selections[0], {
    message: 'Focus on the Windows path.',
    options: { preserveRequested: true },
  })

  await runtime.queueSessionMessage('session-1', {
    message: 'Then update the tests.',
    behavior: 'followUp',
  })
  assert.deepEqual(calls[1].options, {
    images: [],
    streamingBehavior: 'followUp',
    source: 'interactive',
  })

  session.isStreaming = false
  await assert.rejects(
    runtime.queueSessionMessage('session-1', { message: 'Too late.' }),
    /已经结束运行/,
  )
})

test('session input API delegates queued messages without opening another SSE response', async () => {
  const calls = []
  const runtime = {
    async queueSessionMessage(id, input) {
      calls.push({ id, input })
      return { queued: true, behavior: input.behavior, pendingMessageCount: 1, queuedInputs: [] }
    },
  }
  const handler = createApiHandler(runtime)
  const res = response()
  assert.equal(
    await handler(
      request('POST', {
        message: 'Keep going, but skip packaging.',
        attachments: [{ kind: 'path', name: 'notes.md', path: '/workspace/notes.md' }],
        behavior: 'steer',
      }),
      res,
      new URL('http://localhost/api/sessions/session%201/input'),
    ),
    true,
  )
  assert.equal(res.status, 200)
  assert.deepEqual(JSON.parse(res.body), {
    queued: true,
    behavior: 'steer',
    pendingMessageCount: 1,
    queuedInputs: [],
  })
  assert.deepEqual(calls, [
    {
      id: 'session 1',
      input: {
        message: 'Keep going, but skip packaging.',
        attachments: [{ kind: 'path', name: 'notes.md', path: '/workspace/notes.md' }],
        behavior: 'steer',
      },
    },
  ])
})

async function sdkQueue(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-queue-withdraw-'))
  const extensions = { extensions: [], errors: [], runtime: createExtensionRuntime() }
  const { session } = await createAgentSession({
    cwd: directory,
    agentDir: directory,
    model: {
      id: 'queue-fixture',
      provider: 'fixture',
      api: 'openai-completions',
      name: 'Queue fixture',
      input: ['text', 'image'],
      reasoning: false,
      contextWindow: 128000,
      maxTokens: 4096,
    },
    modelRuntime: {},
    tools: [],
    sessionManager: SessionManager.inMemory(directory),
    settingsManager: SettingsManager.inMemory({ compaction: { enabled: false } }),
    resourceLoader: {
      getExtensions: () => extensions,
      getSkills: () => ({ skills: [], diagnostics: [] }),
      getPrompts: () => ({ prompts: [], diagnostics: [] }),
      getThemes: () => ({ themes: [], diagnostics: [] }),
      getAgentsFiles: () => ({ agentsFiles: [] }),
      getSystemPrompt: () => 'Queue fixture',
      getAppendSystemPrompt: () => [],
      extendResources() {},
      async reload() {},
    },
  })
  session._isAgentRunActive = true
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.sessions.set(session.sessionId, {
    session,
    cwd: directory,
    pendingUserMessage: 'current task',
  })
  runtime.selectToolsForMessage = () => {}
  runtime.archiveAttachments = async () => []
  const events = []
  runtime.agentEmitters.set(session.sessionId, (event, data) => events.push({ event, data }))
  runtime.liveSessions.set(session.sessionId, { streaming: true, queuedInputs: [] })
  t.after(async () => {
    session._isAgentRunActive = false
    session.dispose()
    runtime.sessions.clear()
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  const enqueue = (message, behavior = 'steer', attachments = []) =>
    runtime.queueSessionMessage(session.sessionId, { message, behavior, attachments })
  return { runtime, session, events, enqueue }
}

test('withdrawal removes only the selected duplicate and preserves both queue modes and attachments', async (t) => {
  const { runtime, session, enqueue, events } = await sdkQueue(t)
  const attachments = [
    { id: 'notes', kind: 'text', name: 'notes.md', text: 'Original attachment text.' },
    { id: 'diagram', kind: 'image', name: 'diagram.png', mimeType: 'image/png', data: 'aW1hZ2U=' },
  ]
  const first = await enqueue('Same text', 'steer', attachments)
  const second = await enqueue('Same text', 'steer', attachments)
  const followUp = await enqueue('Next step', 'followUp')
  assert.notEqual(first.inputId, second.inputId)
  assert.ok(first.inputId)
  assert.equal(first.queuedInputs[0].text, 'Same text')
  const keptSteer = session.agent.steeringQueue.messages[0]
  const keptFollowUp = session.agent.followUpQueue.messages[0]

  const removed = runtime.withdrawSessionMessage(session.sessionId, second.inputId)
  assert.equal(removed.removed, true)
  assert.deepEqual(removed.withdrawnInput, { text: 'Same text', attachments })
  assert.deepEqual(
    removed.queuedInputs.map((item) => item.id),
    [first.inputId, followUp.inputId],
  )
  assert.equal(session.agent.steeringQueue.messages[0], keptSteer)
  assert.equal(session.agent.followUpQueue.messages[0], keptFollowUp)
  assert.deepEqual(keptSteer.content[1], { type: 'image', data: 'aW1hZ2U=', mimeType: 'image/png' })
  assert.equal(session.pendingMessageCount, 2)
  assert.equal(session.getSteeringMessages().length, 1)
  assert.ok(removed.queueRevision > followUp.queueRevision)
  assert.equal(events.at(-1).data.removedInputId, second.inputId)
  assert.deepEqual(events.at(-1).data.queuedInputs, removed.queuedInputs)
  assert.equal(runtime.sessions.get(session.sessionId).pendingUserMessage, 'current task')
  assert.equal(
    session.sessionManager.getBranch().filter((entry) => entry.type === 'message').length,
    0,
  )
  assert.equal(runtime.withdrawSessionMessage(session.sessionId, second.inputId).removed, false)
  assert.equal(session.pendingMessageCount, 2)

  assert.equal(runtime.withdrawSessionMessage(session.sessionId, followUp.inputId).removed, true)
  assert.equal(session.agent.followUpQueue.messages.length, 0)
  assert.equal(session.getFollowUpMessages().length, 0)
})

test('withdrawal preserves hidden system inputs and refuses identifiers belonging to another session', async (t) => {
  const { runtime, session, enqueue } = await sdkQueue(t)
  const hidden = '[Pisper internal goal continuation]\nContinue the active goal.'
  await session.steer(hidden)
  const first = await enqueue('Visible request')
  const hiddenMessage = session.agent.steeringQueue.messages[0]
  assert.equal(first.queuedInputs.length, 1)
  const other = await sdkQueue(t)
  const foreign = await other.enqueue('Other session request')
  assert.equal(runtime.withdrawSessionMessage(session.sessionId, foreign.inputId).removed, false)
  assert.equal(runtime.withdrawSessionMessage(session.sessionId, first.inputId).removed, true)
  assert.deepEqual(session.agent.steeringQueue.messages, [hiddenMessage])
  assert.deepEqual(session.getSteeringMessages(), [hidden])
  assert.equal(other.session.pendingMessageCount, 1)
})

test('already drained duplicates cannot be withdrawn before their message_start notification', async (t) => {
  const { runtime, session, enqueue } = await sdkQueue(t)
  const first = await enqueue('Same text')
  const second = await enqueue('Same text')
  const [consumed] = session.agent.steeringQueue.drain()
  assert.equal(consumed.content[0].text, 'Same text')
  assert.equal(session.getSteeringMessages().length, 2)
  assert.deepEqual(
    queuedSessionInputs(session).map((item) => item.id),
    [second.inputId],
  )
  assert.equal(runtime.withdrawSessionMessage(session.sessionId, first.inputId).removed, false)
  assert.equal(session.agent.steeringQueue.messages.length, 1)
  assert.equal(runtime.withdrawSessionMessage(session.sessionId, second.inputId).removed, true)
  assert.deepEqual(session.getSteeringMessages(), ['Same text'])
  assert.deepEqual(session.agent.steeringQueue.messages, [])
})

test('queue revisions and live projection follow actual consumption and clearQueue without stale cache', async (t) => {
  const { runtime, session, enqueue } = await sdkQueue(t)
  runtime.streamProjection.getSessionMessagePage = async () => ({
    messages: [],
    pageInfo: {},
    model: 'fixture/queue-fixture',
  })
  const first = await enqueue('First')
  const initial = await runtime.getSessionLive(session.sessionId)
  assert.equal(initial.queuedInputs[0].id, first.inputId)
  const second = await enqueue('Second')
  assert.ok(second.queueRevision > initial.queueRevision)
  session.agent.steeringQueue.drain()
  const consumed = await runtime.getSessionLive(session.sessionId)
  assert.deepEqual(
    consumed.queuedInputs.map((item) => item.id),
    [second.inputId],
  )
  assert.ok(consumed.queueRevision > second.queueRevision)
  assert.equal(sessionInputQueueRevision(session), consumed.queueRevision)
  session.clearQueue()
  const cleared = await runtime.getSessionLive(session.sessionId)
  assert.deepEqual(cleared.queuedInputs, [])
  assert.ok(cleared.queueRevision > consumed.queueRevision)
  assert.equal(runtime.withdrawSessionMessage(session.sessionId, second.inputId).removed, false)
})

test('concurrent prompt transformations preserve each request original payload for withdrawal', async (t) => {
  const { session } = await sdkQueue(t)
  let releaseFirst
  const firstWait = new Promise((resolve) => {
    releaseFirst = resolve
  })
  const firstOriginal = {
    text: 'First original',
    attachments: [{ kind: 'text', text: 'First data' }],
  }
  const secondOriginal = {
    text: 'Second original',
    attachments: [{ kind: 'text', text: 'Second data' }],
  }
  const first = captureQueuedSessionInput(session, 'steer', firstOriginal, async () => {
    await firstWait
    await session.prompt('Transformed text', { streamingBehavior: 'steer' })
  })
  const secondId = await captureQueuedSessionInput(session, 'steer', secondOriginal, () =>
    session.prompt('Transformed text', { streamingBehavior: 'steer' }),
  )
  releaseFirst()
  const firstId = await first
  assert.notEqual(firstId, secondId)
  assert.deepEqual(withdrawQueuedSessionInput(session, firstId), firstOriginal)
  assert.deepEqual(withdrawQueuedSessionInput(session, secondId), secondOriginal)
})

test('an unsupported or inconsistent SDK queue fails before changing pending messages', async (t) => {
  const { session, enqueue } = await sdkQueue(t)
  const first = await enqueue('Keep this')
  session._steeringMessages = []
  assert.throws(() => withdrawQueuedSessionInput(session, first.inputId), /状态不一致/)
  assert.equal(session.agent.steeringQueue.messages.length, 1)
  assert.throws(() => withdrawQueuedSessionInput({}, first.inputId), /引擎版本不支持/)
})

test('queueing rechecks run state after asynchronous attachment preparation', async (t) => {
  const { runtime, session, enqueue } = await sdkQueue(t)
  runtime.preparePromptAttachments = async () => {
    session._isAgentRunActive = false
    return { images: [], contexts: [] }
  }
  await assert.rejects(enqueue('Too late'), /已经结束运行/)
  assert.deepEqual(session.agent.steeringQueue.messages, [])
})

test('DELETE input API decodes stable identifiers and keeps the ongoing run intact', async () => {
  const calls = []
  const runtime = {
    withdrawSessionMessage(sessionId, inputId) {
      calls.push({ sessionId, inputId })
      return {
        removed: true,
        inputId,
        queuedInputs: [],
        queueRevision: 12,
        pendingMessageCount: 0,
        withdrawnInput: { text: 'Restore this', attachments: [] },
      }
    },
  }
  const res = response()
  const handler = createApiHandler(runtime)
  assert.equal(
    await handler(
      request('DELETE'),
      res,
      new URL('http://localhost/api/sessions/session%201/input/input%202'),
    ),
    true,
  )
  assert.equal(res.status, 200)
  assert.deepEqual(calls, [{ sessionId: 'session 1', inputId: 'input 2' }])
  assert.equal(JSON.parse(res.body).removed, true)
  assert.equal(JSON.parse(res.body).withdrawnInput.text, 'Restore this')
})

test('withdrawal API returns strict Unicode JSON and reports a consumed input without success', async (t) => {
  const { runtime, session, enqueue } = await sdkQueue(t)
  const queued = await enqueue('Unicode \ud800 request')
  const handler = createApiHandler(runtime)
  const url = new URL(`http://localhost/api/sessions/${session.sessionId}/input/${queued.inputId}`)
  const res = response()
  await handler(request('DELETE'), res, url)
  assert.equal(res.status, 200)
  assert.doesNotMatch(res.body, /\\ud800/i)
  assert.equal(JSON.parse(res.body).removed, true)
  const repeated = response()
  await handler(request('DELETE'), repeated, url)
  assert.equal(repeated.status, 200)
  assert.equal(JSON.parse(repeated.body).removed, false)
  assert.equal(JSON.parse(repeated.body).withdrawnInput, undefined)
})
