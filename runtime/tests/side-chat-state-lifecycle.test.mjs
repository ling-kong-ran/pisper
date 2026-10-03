import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import * as sessionState from '../../src/lib/session/session-state.ts'
import { shouldPollLiveSession } from '../../src/features/chat/model/live-session-sync.ts'

const code = transformSync(await readFile('src/features/chat/hooks/use-session-catalog.ts', 'utf8'), {
  loader: 'ts',
  format: 'cjs',
}).code
const SIDE_ID = 'side-retired'
function runningState() {
  return {
    ...sessionState.DEFAULT_SESSION_STATE,
    messages: [{ id: 'user', role: 'user', text: 'temporary message' }],
    streaming: true,
    recovering: true,
    approvals: [{ id: 'permission' }],
    agents: [{ id: 'child', status: 'running' }],
  }
}
function fixture() {
  const modules = {
    '@tanstack/react-query': { useQueryClient: () => ({ setQueryData: () => {} }) },
    react: {
      useState: (value) => [typeof value === 'function' ? value() : value, () => {}],
      useRef: (value) => ({ current: value }),
      useCallback: (callback) => callback,
      useEffect: () => {},
    },
    '@/app/brand': { APP_NAME: 'Pisper' },
    '@/app/storage': { STORAGE_KEYS: { activeSession: 'test-active' } },
    '@/app/i18n/use-i18n': { useI18n: () => ({ t: (key) => key }) },
    '@/lib/storage/page-state-storage': {
      pageStateStorage: { getItem: () => null, setItem: () => {}, removeItem: () => {} },
    },
    '@/lib/session/session-state': sessionState,
    '@/lib/session/plan-protocol': {},
    '@/features/chat/api/chat-api': {},
    './chat-errors': {},
    '@/features/chat/model/chat-errors': {},
    './events': {},
    './session-list': { createSessionTitleReconciler: () => ({}) },
  }
  modules['@/features/chat/model/events'] = modules['./events']
  modules['@/features/chat/model/chat-errors'] = modules['./chat-errors']
  modules['@/features/chat/model/session-list'] = modules['./session-list']
  const module = { exports: {} }
  runInNewContext(code, {
    module,
    exports: module.exports,
    localStorage: { getItem: () => null },
    require(id) {
      assert.ok(Object.hasOwn(modules, id), id)
      return modules[id]
    },
  })
  return module.exports.useSessionCatalog({ notify: () => {} })
}

test('authoritative expiration discards even retained, streaming, recovering and agent-active side state', () => {
  const catalog = fixture()
  catalog.updateSessions([{ id: 'main', name: 'Main chat' }])
  catalog.updateSessionState(SIDE_ID, runningState())
  const release = catalog.retainSessionState(SIDE_ID)
  assert.equal(catalog.releaseSessionState(SIDE_ID), false)
  assert.equal(shouldPollLiveSession(catalog.getSessionState(SIDE_ID)), true)
  let notifications = 0
  const unsubscribe = catalog.subscribeSessionState(SIDE_ID, () => {
    notifications += 1
  })
  catalog.discardSessionState(SIDE_ID)
  assert.equal(catalog.getSessionState(SIDE_ID), sessionState.DEFAULT_SESSION_STATE)
  assert.equal(Object.hasOwn(catalog.sessionStatesRef.current, SIDE_ID), false)
  assert.equal(shouldPollLiveSession(catalog.getSessionState(SIDE_ID)), false)
  assert.equal(notifications, 1)
  release()
  assert.equal(notifications, 1)
  assert.deepEqual(catalog.sessionsRef.current, [{ id: 'main', name: 'Main chat' }])
  unsubscribe()
})

test('late SSE, messages and live snapshots cannot recreate an expired side ID', () => {
  const catalog = fixture()
  const oldState = runningState()
  catalog.updateSessionState(SIDE_ID, oldState)
  catalog.discardSessionState(SIDE_ID)
  let updaterCalled = false
  catalog.updateSessionState(SIDE_ID, () => {
    updaterCalled = true
    return oldState
  })
  catalog.updateSessionState(SIDE_ID, { streaming: true, approvals: [{ id: 'late-permission' }] })
  catalog.replaceSessionStates({
    [SIDE_ID]: oldState,
    main: { ...sessionState.DEFAULT_SESSION_STATE, error: 'kept' },
  })
  assert.equal(updaterCalled, false)
  assert.equal(Object.hasOwn(catalog.sessionStatesRef.current, SIDE_ID), false)
  assert.equal(catalog.getSessionState(SIDE_ID), sessionState.DEFAULT_SESSION_STATE)
  assert.equal(catalog.getSessionState('main').error, 'kept')
  catalog.updateSessionState('side-new-uuid', runningState())
  assert.equal(catalog.getSessionState('side-new-uuid').streaming, true)
  assert.equal(catalog.getSessionState('side-new-uuid').messages[0].text, 'temporary message')
})

test('ordinary panel closure releases its lease without discarding an ongoing side chat', () => {
  const catalog = fixture()
  catalog.updateSessionState(SIDE_ID, runningState())
  const release = catalog.retainSessionState(SIDE_ID)
  release()
  release()
  assert.equal(catalog.getSessionState(SIDE_ID).streaming, true)
  assert.equal(shouldPollLiveSession(catalog.getSessionState(SIDE_ID)), true)
  catalog.updateSessionState(SIDE_ID, {
    messages: [{ id: 'agent', role: 'agent', text: 'continues' }],
  })
  assert.equal(catalog.getSessionState(SIDE_ID).messages[0].text, 'continues')
})
