import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService, runPromptWithAbortGuard } from '../runtime/agent-runtime.mjs'

function liveState(streaming = false) {
  return {
    streaming,
    text: '',
    thinkingText: '',
    tools: [],
    assets: [],
    error: '',
    activityFeed: [],
    currentActivity: null,
    startedAt: null,
    finishedAt: null,
    lastActivityAt: new Date().toISOString(),
  }
}

function mockSession({
  setSessionName = () => {},
  abort = async () => {},
  dispose = () => {},
} = {}) {
  return {
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    messages: [],
    setSessionName,
    setActiveToolsByName: () => {},
    clearQueue: () => {},
    abort,
    dispose,
  }
}

function freshRuntime(directory) {
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = {
    getGlobalSettings: () => ({
      defaultProvider: 'openai',
      defaultModel: 'gpt-5.4',
      defaultThinkingLevel: 'medium',
    }),
  }
  runtime.listStoredSessions = async () => []
  return runtime
}

function seedRunningSession(runtime, id, session) {
  const value = {
    cwd: runtime.cwd,
    name: '运行中会话',
    session,
    runActive: true,
  }
  runtime.sessions.set(id, value)
  runtime.liveSessions.set(id, liveState(true))
  return value
}

test('abort guard force-settles a hung prompt after the abort deadline', async () => {
  const value = { abortForceTimeoutMs: 120 }
  value.abortedAt = Date.now()
  await assert.rejects(
    runPromptWithAbortGuard(value, () => new Promise(() => {})),
    /强制中断/,
  )
})

test('abort guard disposes the agent session so hung tools are terminated', async () => {
  let disposed = 0
  const value = {
    abortForceTimeoutMs: 120,
    session: { dispose: () => (disposed += 1) },
  }
  value.abortedAt = Date.now()
  await assert.rejects(
    runPromptWithAbortGuard(value, () => new Promise(() => {})),
    /强制中断/,
  )
  assert.equal(disposed, 1, 'the agent session must be disposed to kill hung tools')
  assert.equal(value.forceDisposed, true, 'the resident must be flagged for cleanup')
})

test('abort guard passes through a normal prompt result', async () => {
  const value = { abortForceTimeoutMs: 120 }
  assert.equal(await runPromptWithAbortGuard(value, () => Promise.resolve('ok')), 'ok')
  await assert.rejects(
    runPromptWithAbortGuard(value, () => Promise.reject(new Error('model-error'))),
    /model-error/,
  )
})

test('abort guard waits forever for a hung prompt when no abort happened', async () => {
  const value = { abortForceTimeoutMs: 60 }
  let settled = false
  const guard = runPromptWithAbortGuard(value, () => new Promise(() => {})).then(
    () => (settled = true),
    () => (settled = true),
  )
  await new Promise((resolve) => setTimeout(resolve, 200))
  assert.equal(settled, false, 'without an abort the guard must keep waiting')
  value.abortedAt = Date.now()
  await guard
  assert.equal(settled, true, 'after the abort deadline the guard must settle')
})

test('abortSession records the abort deadline on the resident value', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-abort-deadline-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = freshRuntime(directory)
  const id = 'session-abort-deadline'
  const value = seedRunningSession(runtime, id, mockSession())
  await runtime.abortSession(id)
  assert.ok(value.abortedAt, 'abortedAt must be recorded for the force-settle guard')
})

test('a new prompt clears interruption markers left by the previous run', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-abort-reset-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = freshRuntime(directory)
  const id = 'session-abort-reset'
  const value = {
    cwd: runtime.cwd,
    name: '中断后继续',
    session: { ...mockSession(), sessionId: id },
    abortedAt: Date.now() - 60_000,
  }
  runtime.sessions.set(id, value)
  let observed
  runtime.runSessionPrompt = async (current) => {
    observed = {
      abortedAt: current.abortedAt,
      forceDisposed: current.forceDisposed,
    }
  }

  await runtime.streamPrompt({ sessionId: id, message: '继续发送', send: () => {} })

  assert.deepEqual(observed, { abortedAt: undefined, forceDisposed: undefined })
  assert.equal(Object.hasOwn(value, 'abortedAt'), false)
  assert.equal(Object.hasOwn(value, 'forceDisposed'), false)
})

test('a stale force-disposed resident is rebuilt before the next prompt', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-abort-rebuild-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = freshRuntime(directory)
  const id = 'session-abort-rebuild'
  let disposed = 0
  const stale = {
    cwd: runtime.cwd,
    name: '待恢复会话',
    session: {
      ...mockSession({ dispose: () => (disposed += 1) }),
      sessionId: id,
      isStreaming: true,
    },
    forceDisposed: true,
  }
  const replacement = {
    cwd: runtime.cwd,
    name: stale.name,
    session: { ...mockSession(), sessionId: id },
  }
  runtime.sessions.set(id, stale)
  let lookups = 0
  runtime.getOrCreateSession = async () => {
    lookups += 1
    if (lookups === 1) return stale
    runtime.sessions.set(id, replacement)
    return replacement
  }
  let prompted
  runtime.runSessionPrompt = async (current) => {
    prompted = current
  }

  await runtime.streamPrompt({ sessionId: id, message: '恢复发送', send: () => {} })

  assert.equal(disposed, 1)
  assert.equal(lookups, 2)
  assert.equal(prompted, replacement)
  assert.equal(runtime.sessions.get(id), replacement)
})

test('forced interruption releases a disposed resident with stale streaming state', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-abort-release-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = freshRuntime(directory)
  const id = 'session-abort-release'
  let disposed = 0
  const session = {
    ...mockSession({ dispose: () => (disposed += 1) }),
    sessionId: id,
  }
  const value = { cwd: runtime.cwd, name: '强制中断', session }
  runtime.sessions.set(id, value)
  runtime.runSessionPrompt = async (current) => {
    current.forceDisposed = true
    current.session.isStreaming = true
  }

  await runtime.streamPrompt({ sessionId: id, message: '触发强制中断', send: () => {} })

  assert.equal(disposed, 1)
  assert.equal(runtime.sessions.has(id), false, 'a disposed resident must never be reused')
})

test('aborting a running session preserves the session and never creates a new one', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-abort-keep-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = freshRuntime(directory)
  let aborted = false
  const id = 'session-abort'
  seedRunningSession(runtime, id, mockSession({ abort: async () => (aborted = true) }))

  const result = await runtime.abortSession(id)
  assert.equal(result, true)
  assert.equal(aborted, true, 'session abort must be invoked')
  assert.ok(runtime.sessions.has(id), 'aborted session must stay resident')

  const sessions = await runtime.listSessions()
  assert.ok(
    sessions.some((session) => session.id === id),
    'aborted session must stay listed',
  )
  assert.equal(
    sessions.length,
    1,
    'no implicit new session may appear after aborting (frontend auto-create is a symptom of a lost session)',
  )
})

test('aborting an unknown session is a no-op that does not invent sessions', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-abort-noop-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = freshRuntime(directory)
  const sessions = await runtime.listSessions()
  assert.equal(sessions.length, 0)
  assert.equal(await runtime.abortSession('does-not-exist'), false)
  assert.equal((await runtime.listSessions()).length, 0, 'abort must never create a session')
})

test('renaming a running session is rejected and keeps the session intact', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-rename-keep-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = freshRuntime(directory)
  const id = 'session-rename'
  const value = seedRunningSession(runtime, id, mockSession())

  await assert.rejects(() => runtime.renameSession(id, '新标题'), /正在运行/)
  assert.equal(runtime.sessions.get(id), value, 'resident runtime must survive a rejected rename')
  assert.equal(runtime.sessionRunIsActive(id), true, 'run state must survive a rejected rename')
})

test('a running session switches execution mode without dropping its active runtime', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mode-keep-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = freshRuntime(directory)
  const id = 'session-mode'
  const value = seedRunningSession(runtime, id, mockSession())
  assert.equal(
    runtime.sessionRunIsActive(id),
    true,
    'runActive + live.streaming must mark the session as active',
  )

  const mode = await runtime.setSessionExecutionMode(id, 'read-only')
  assert.deepEqual(mode, {
    id,
    executionMode: 'read-only',
    permissionMode: 'ask',
  })
  assert.equal(runtime.sessions.get(id), value, 'resident runtime must be preserved')
  assert.equal(runtime.sessionRunIsActive(id), true)
  assert.equal(runtime.getSessionExecutionMode(id), 'read-only')
  assert.equal(value.runtimeVersion, -1, 'the next prompt must rebuild the mode-specific tools')

  await assert.rejects(() => runtime.setSessionPermission(id, 'ask'), /运行/)
  assert.equal(runtime.sessions.get(id), value, 'permission change must not drop the runtime')
})

test('switching execution mode on an idle session keeps the session listed and readable', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mode-idle-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = freshRuntime(directory)
  const id = 'session-idle-mode'
  const session = mockSession()
  const value = { cwd: runtime.cwd, name: '待切换', session }
  runtime.sessions.set(id, value)
  runtime.liveSessions.set(id, liveState(false))
  runtime.listStoredSessions = async () => [
    {
      id,
      name: '待切换',
      firstMessage: '',
      messageCount: 1,
      cwd: runtime.cwd,
      created: new Date(0),
      modified: new Date(),
    },
  ]

  const mode = await runtime.setSessionExecutionMode(id, 'read-only')
  assert.equal(mode.executionMode, 'read-only')
  assert.ok(
    !runtime.sessions.has(id),
    'idle resident runtime may be disposed after the mode switch (rebuilt from disk)',
  )
  assert.ok(
    (await runtime.listSessions()).some((item) => item.id === id),
    'session must stay listed after the mode switch',
  )
})
