import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import { ApiError } from '../../src/lib/http/api-error.ts'
import * as sessionState from '../../src/lib/session/session-state.ts'
import * as planProtocol from '../../src/lib/session/plan-protocol.ts'
import * as runActivity from '../../src/features/chat/model/run-activity.ts'
import * as chatErrors from '../../src/features/chat/model/chat-errors.ts'

const [commandCode, syncCode] = await Promise.all(
  ['use-session-commands', 'use-live-session-sync'].map(
    async (name) =>
      transformSync(await readFile(`src/features/chat/hooks/${name}.ts`, 'utf8'), {
        loader: 'ts',
        format: 'cjs',
      }).code,
  ),
)
const approval = { id: 'approval-1', toolName: 'write', args: {}, reason: 'Confirm edit' }

function fixture({ resolveApproval, getLiveSession }) {
  const sessionId = 'side-approval-test'
  const sessionStatesRef = {
    current: {
      [sessionId]: {
        ...sessionState.DEFAULT_SESSION_STATE,
        messages: [{ id: 'user-1', role: 'user', text: 'edit' }],
        approvals: [approval],
        streaming: true,
        loaded: true,
        runStartedAt: '2026-09-27T00:00:00.000Z',
      },
    },
  }
  const localStreamSessionsRef = { current: new Set([sessionId]) }
  const streamGenerationRef = { current: new Map([[sessionId, 1]]) }
  const updateSessionState = (id, update) => {
    sessionStatesRef.current[id] = sessionState.applySessionUpdate(
      sessionStatesRef.current[id],
      update,
    )
  }
  const updateSessions = (update) => (typeof update === 'function' ? update([]) : update)
  const requests = []
  const notices = []
  const modules = {
    react: {
      useCallback: (callback) => callback,
      useEffect: () => {},
      useRef: (value) => ({ current: value }),
      useState: (value) => [value, () => {}],
    },
    '@/app/i18n/use-i18n': { useI18n: () => ({ t: (key) => key, language: 'en-US' }) },
    '@/lib/format/format': {},
    '@/lib/http/http': { ApiError },
    '@/lib/platform/pick-system-directory': {},
    '@/lib/session/plan-protocol': planProtocol,
    '@/lib/session/session-state': sessionState,
    './chat-errors': chatErrors,
    './events': {},
    './session-runtime-selections': {},
    './live-session-sync': {},
    './run-activity': runActivity,
    './use-session-catalog': { FOCUS_MESSAGE_PAGE_SIZE: 40 },
    './voice-response-stream': { publishVoiceSnapshot: () => {} },
    './chat-api': {
      chatApi: {
        resolveApproval,
        getLiveSession: async (id) => {
          requests.push(id)
          return getLiveSession(id)
        },
      },
    },
  }
  const evaluate = (code) => {
    const module = { exports: {} }
    runInNewContext(code, {
      module,
      exports: module.exports,
      require: (id) => {
        if (id.startsWith('@/features/chat/')) {
        const parts = id.split('/')
        const shortId = './' + parts[parts.length - 1]
        if (modules[shortId] !== undefined) return modules[shortId]
      }
      assert.ok(modules[id], id)
        return modules[id]
      },
    })
    return module.exports
  }
  const { syncLiveSession } = evaluate(syncCode).useLiveSessionSync({
    sessionStates: sessionStatesRef.current,
    sessionStatesRef,
    localStreamSessionsRef,
    streamGenerationRef,
    updateSessionState,
    updateSessions,
  })
  const commands = evaluate(commandCode).useSessionCommands({
    sessionStatesRef,
    updateSessionState,
    updateSessions,
    syncLiveSession,
    notify: (...args) => notices.push(args),
  })
  return {
    sessionId,
    commands,
    requests,
    notices,
    localStreamSessionsRef,
    streamGenerationRef,
    get state() {
      return sessionStatesRef.current[sessionId]
    },
    update: (update) => updateSessionState(sessionId, update),
  }
}
function live(approvals = [approval]) {
  return { streaming: true, messages: [{ id: 'user-1', role: 'user', text: 'edit' }], approvals }
}

test('failed side-chat approval synchronizes despite a locally owned stream and keeps the pending action', async () => {
  const failure = new ApiError('temporary transport failure', { kind: 'network' })
  const f = fixture({
    resolveApproval: async () => {
      throw failure
    },
    getLiveSession: async () => live(),
  })
  await assert.rejects(
    f.commands.resolveToolApproval(f.sessionId, approval.id, true),
    (error) => error === failure,
  )
  assert.deepEqual(f.requests, [f.sessionId])
  assert.deepEqual(f.state.approvals, [approval])
  assert.equal(f.state.streaming, true)
  assert.equal(f.localStreamSessionsRef.current.has(f.sessionId), false)
  assert.equal(f.streamGenerationRef.current.get(f.sessionId), 2)
  assert.equal(f.state.error, failure.message)
})

test('approval stays retryable if both its POST and recovery snapshot fail', async () => {
  const failure = new Error('offline')
  const f = fixture({
    resolveApproval: async () => {
      throw failure
    },
    getLiveSession: async () => {
      throw failure
    },
  })
  await assert.rejects(
    f.commands.resolveToolApproval(f.sessionId, approval.id, false),
    (error) => error === failure,
  )
  assert.deepEqual(f.state.approvals, [approval])
  // 没有拿到快照时不能使原 SSE 失去写入资格。
  assert.equal(f.localStreamSessionsRef.current.has(f.sessionId), true)
  assert.equal(f.streamGenerationRef.current.get(f.sessionId), 1)
})

test('a resolved snapshot does not revive an approval when its HTTP response was lost', async () => {
  const f = fixture({
    resolveApproval: async () => {
      throw new Error('response lost')
    },
    getLiveSession: async () => live([]),
  })
  await assert.rejects(f.commands.resolveToolApproval(f.sessionId, approval.id, true))
  assert.equal(f.state.approvals.length, 0)
})

test('a concurrent resolved event remains authoritative while an approval POST is pending', async () => {
  let finish
  const response = new Promise((resolve) => {
    finish = resolve
  })
  const f = fixture({ resolveApproval: () => response, getLiveSession: async () => live([]) })
  const pending = f.commands.resolveToolApproval(f.sessionId, approval.id, true)
  assert.deepEqual(f.state.approvals, [approval])
  f.update({ approvals: [{ ...approval, id: 'approval-2' }] })
  finish({ found: true })
  await pending
  assert.equal(f.state.approvals.length, 1)
  assert.equal(f.state.approvals[0].id, 'approval-2')
  assert.equal(f.requests.length, 0)
})
