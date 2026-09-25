import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import * as sessionState from '../../src/lib/session-state.ts'
import * as runActivity from '../../src/features/chat/run-activity.ts'
import { reconcileTerminalStreamState } from '../../src/features/chat/stream-event-dispatch.ts'
import { shouldRevealSessionContext } from '../../src/features/chat/session-context-layout.ts'

const promptCode = transformSync(
  await readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code

function deferred() {
  let resolve
  let reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

// 使用真实 hook 和终态归并，仅替换网络、React 调度及此次未调用的发送依赖。
function fixture({ abort, syncLiveSession = async () => {}, initial = {} }) {
  const notices = []
  const ref = {
    current: {
      session: {
        ...sessionState.DEFAULT_SESSION_STATE,
        streaming: true,
        lifecycle: { phase: 'running' },
        runStartedAt: '2026-09-24T10:00:00.000Z',
        queuedInputRunId: 'run-1',
        runStopped: false,
        ...initial,
      },
    },
  }
  const updateSessionState = (id, update) => {
    ref.current[id] = sessionState.applySessionUpdate(ref.current[id], update)
  }
  const modules = {
    react: { useCallback: (callback) => callback, useRef: (value) => ({ current: value }) },
    '@/app/brand': { APP_NAME: 'Pisper' },
    '@/app/use-i18n': { useI18n: () => ({ t: (key) => key }) },
    '@/lib/session-state': sessionState,
    '@/lib/streaming-ui': {},
    './chat-api': { chatApi: { abort } },
    './chat-errors': {},
    './run-activity': runActivity,
    './stream-event-dispatch': {},
  }
  const module = { exports: {} }
  runInNewContext(promptCode, {
    module,
    exports: module.exports,
    require(id) {
      assert.ok(modules[id], id)
      return modules[id]
    },
  })
  const commands = module.exports.usePromptCommands({
    sessionStatesRef: ref,
    notify: (...notice) => notices.push(notice),
    updateSessionState,
    updateSessions: (update) => (typeof update === 'function' ? update([]) : update),
    syncLiveSession,
  })
  return {
    commands,
    notices,
    update: (update) => updateSessionState('session', update),
    get state() {
      return ref.current.session
    },
    done() {
      updateSessionState('session', (current) =>
        reconcileTerminalStreamState(current, {
          agentId: 'agent',
          responseText: 'finished',
          data: {},
          finishedAt: '2026-09-24T10:00:01.000Z',
        }),
      )
    },
  }
}

test('stop intent suppresses context reveal when done arrives before the abort response', async () => {
  const pending = deferred()
  const f = fixture({ abort: () => pending.promise })
  const before = { sessionId: 'session', streaming: true, completed: false }
  const stopping = f.commands.abort('session')
  assert.equal(f.state.runStopped, true)
  assert.equal(f.state.streaming, true)
  assert.notEqual(
    runActivity.deriveRunActivity({ streaming: f.state.streaming, stopped: f.state.runStopped })
      .stage,
    'stopped',
  )
  f.done()
  assert.equal(f.state.lifecycle.phase, 'completed')
  assert.equal(f.state.runStopped, true)
  assert.equal(
    shouldRevealSessionContext(before, {
      sessionId: 'session',
      streaming: f.state.streaming,
      completed: f.state.lifecycle.phase === 'completed' && !f.state.error && !f.state.runStopped,
    }),
    false,
  )
  pending.resolve({ aborted: true })
  await stopping
  assert.equal(f.state.runStopped, true)
  assert.equal(f.notices.length, 1)
})

test('duplicate stop requests share one request and failed requests restore the original state', async () => {
  const pending = deferred()
  let calls = 0
  const f = fixture({
    abort: () => {
      calls += 1
      return calls === 1 ? pending.promise : Promise.resolve({ aborted: true })
    },
  })
  const stopping = f.commands.abort('session')
  assert.equal(f.commands.abort('session'), stopping)
  assert.equal(calls, 1)
  pending.reject(new Error('offline'))
  await assert.rejects(stopping, /offline/)
  assert.equal(f.state.runStopped, false)
  assert.equal(f.state.streaming, true)
  await f.commands.abort('session')
  assert.equal(calls, 2)
  assert.equal(f.state.runStopped, true)
})

test('failed stop requests preserve an existing stopped state', async () => {
  const f = fixture({
    abort: async () => {
      throw new Error('offline')
    },
    initial: { runStopped: true },
  })
  await assert.rejects(f.commands.abort('session'), /offline/)
  assert.equal(f.state.runStopped, true)
})

test('failed live refresh cannot undo an accepted stop request', async () => {
  const f = fixture({
    abort: async () => ({ aborted: true }),
    syncLiveSession: async () => {
      throw new Error('snapshot unavailable')
    },
  })
  await assert.rejects(f.commands.abort('session'), /snapshot unavailable/)
  assert.equal(f.state.runStopped, true)
  f.done()
  assert.equal(f.state.runStopped, true)
})

test('late stop failures cannot clear the stop intent of a newer run', async () => {
  const pending = deferred()
  const f = fixture({ abort: () => pending.promise })
  const stopping = f.commands.abort('session')
  f.update({
    runStartedAt: '2026-09-24T10:01:00.000Z',
    queuedInputRunId: 'run-2',
    runStopped: true,
  })
  pending.reject(new Error('old request failed'))
  await assert.rejects(stopping, /old request failed/)
  assert.equal(f.state.runStopped, true)
})

test('late stop success cannot settle a newer run or request its live snapshot', async () => {
  const pending = deferred()
  let snapshots = 0
  const f = fixture({
    abort: () => pending.promise,
    syncLiveSession: async () => {
      snapshots += 1
    },
  })
  const stopping = f.commands.abort('session')
  f.update({
    runStartedAt: '2026-09-24T10:01:00.000Z',
    queuedInputRunId: 'run-2',
    runStopped: false,
  })
  pending.resolve({ aborted: true })
  await stopping
  assert.equal(f.state.streaming, true)
  assert.equal(f.state.runStopped, false)
  assert.equal(snapshots, 0)
  assert.equal(f.notices.length, 0)
})

test('server timestamp reconciliation retains the local run identity during stop settlement', async () => {
  const f = fixture({
    abort: async () => ({ aborted: true }),
    syncLiveSession: async () => {
      f.update({ runStartedAt: '2026-09-24T10:00:00.125Z' })
    },
  })
  await f.commands.abort('session')
  assert.equal(f.state.streaming, false)
  assert.equal(f.state.runStopped, true)
  assert.equal(f.notices.length, 1)
})

test('server timestamp reconciliation still allows failed stop intent to roll back', async () => {
  const pending = deferred()
  const f = fixture({ abort: () => pending.promise })
  const stopping = f.commands.abort('session')
  f.update({ runStartedAt: '2026-09-24T10:00:00.125Z' })
  pending.reject(new Error('offline'))
  await assert.rejects(stopping, /offline/)
  assert.equal(f.state.runStopped, false)
})

test('restored runs without a local run ID restore stop intent after a failed request', async () => {
  const f = fixture({
    abort: async () => {
      throw new Error('offline')
    },
    initial: { queuedInputRunId: undefined },
  })
  await assert.rejects(f.commands.abort('session'), /offline/)
  assert.equal(f.state.runStopped, false)
})

test('a newer run can be stopped while the previous run stop request remains pending', async () => {
  const first = deferred()
  const second = deferred()
  let calls = 0
  const f = fixture({ abort: () => (++calls === 1 ? first.promise : second.promise) })
  const oldStop = f.commands.abort('session')
  f.done()
  f.update({
    streaming: true,
    runStartedAt: '2026-09-24T10:01:00.000Z',
    queuedInputRunId: 'run-2',
    runStopped: false,
  })
  const newStop = f.commands.abort('session')
  assert.notEqual(newStop, oldStop)
  assert.equal(calls, 2)
  first.resolve({ aborted: true })
  await oldStop
  assert.equal(f.commands.abort('session'), newStop)
  assert.equal(f.state.streaming, true)
  assert.equal(f.state.runStopped, true)
  second.resolve({ aborted: true })
  await newStop
  assert.equal(f.state.streaming, false)
  assert.equal(f.notices.length, 1)
})
