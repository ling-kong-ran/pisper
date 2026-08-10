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

test('a running session rejects an execution-mode switch and survives it', async (t) => {
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

  let outcome = 'pending'
  try {
    const result = await runtime.setSessionExecutionMode(id, 'read-only')
    outcome = `resolved:${JSON.stringify(result)}`
  } catch (error) {
    outcome = `rejected:${error.message}`
  }
  assert.match(outcome, /^rejected:/, `execution-mode switch must be rejected, got ${outcome}`)
  assert.equal(runtime.sessions.get(id), value, 'resident runtime must be preserved')
  assert.equal(runtime.sessionRunIsActive(id), true)

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
