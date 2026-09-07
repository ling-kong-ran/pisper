import assert from 'node:assert/strict'
import { afterEach, test } from 'node:test'
import { QueryObserver } from '@tanstack/react-query'
import { chatApi } from '../../src/features/chat/chat-api.ts'
import {
  fetchStartupQuery,
  installStartupQueryEvents,
  invalidateStartupQuery,
  queryClient,
  startupQueryOptions,
} from '../../src/lib/startup-queries.ts'
import { markStartupPhase } from '../../src/lib/startup-diagnostics.ts'
import { useClientStore } from '../../src/stores/client-store.ts'
import { useRuntimeCapabilitiesStore } from '../../src/stores/runtime-capabilities-store.ts'

const originalFetch = globalThis.fetch
const originalWindow = globalThis.window
const tick = () => new Promise((resolve) => setImmediate(resolve))
const json = (value) => new Response(JSON.stringify(value), { status: 200 })

function deferred() {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}

afterEach(() => {
  queryClient.clear()
  globalThis.fetch = originalFetch
  if (originalWindow === undefined) delete globalThis.window
  else globalThis.window = originalWindow
})

test('shell observers, StrictMode remount and chat readers share pending startup requests', async () => {
  const pending = deferred()
  const calls = []
  globalThis.fetch = async (path) => {
    calls.push(path)
    await pending.promise
    return json(path === '/api/config' ? { providers: [], model: 'old' } : { sessions: [] })
  }
  const config = new QueryObserver(queryClient, startupQueryOptions('config'))
  const sessions = new QueryObserver(queryClient, startupQueryOptions('sessions'))
  const unsubscribe = config.subscribe(() => {})
  unsubscribe()
  const unsubscribeAgain = config.subscribe(() => {})
  const unsubscribeSessions = sessions.subscribe(() => {})
  const reads = Promise.all([
    chatApi.getConfig(),
    chatApi.listSessions(),
    fetchStartupQuery('sessions'),
  ])
  await tick()
  assert.deepEqual(calls.sort(), ['/api/config', '/api/sessions'])
  pending.resolve()
  await reads
  await chatApi.getConfig()
  assert.equal(calls.length, 2)
  unsubscribeAgain()
  unsubscribeSessions()
})

test('a lazy chat page reuses shell snapshots after both startup requests have completed', async () => {
  const calls = []
  globalThis.fetch = async (path) => {
    calls.push(path)
    return json(path === '/api/config' ? { providers: [] } : { sessions: [{ id: 'existing' }] })
  }
  const snapshots = await Promise.all([fetchStartupQuery('sessions'), fetchStartupQuery('config')])
  await tick()
  const chatSnapshots = await Promise.all([
    chatApi.listSessions({ refresh: false }),
    chatApi.getConfig(),
  ])
  assert.deepEqual(chatSnapshots, snapshots)
  assert.deepEqual(calls.sort(), ['/api/config', '/api/sessions'])
})

test('explicit chat refresh remains fresh while ordinary config reads reuse the cache', async () => {
  let revision = 0
  globalThis.fetch = async (path) =>
    json(
      path === '/api/config'
        ? { providers: [], revision: ++revision }
        : { sessions: [], revision: ++revision },
    )
  const first = await chatApi.listSessions()
  const refreshed = await chatApi.listSessions()
  assert.ok(refreshed.revision > first.revision)
  assert.deepEqual(await chatApi.listSessions({ refresh: false }), refreshed)
  const config = await chatApi.getConfig()
  assert.deepEqual(await chatApi.getConfig(), config)
  assert.ok((await chatApi.getConfig({ refresh: true })).revision > config.revision)
})

test('one event listener refreshes all observers and cleanup is StrictMode safe', async () => {
  let revision = 0
  globalThis.fetch = async () => json({ sessions: [], revision: ++revision })
  const target = new EventTarget()
  installStartupQueryEvents(target)()
  const cleanup = installStartupQueryEvents(target)
  const observers = [1, 2, 3].map(
    () => new QueryObserver(queryClient, startupQueryOptions('sessions')),
  )
  const unsubscribes = observers.map((observer) => observer.subscribe(() => {}))
  await fetchStartupQuery('sessions')
  target.dispatchEvent(new Event('pisper:sessions-updated'))
  await fetchStartupQuery('sessions')
  assert.equal(revision, 2)
  assert.equal(queryClient.getQueryData(['sessions']).revision, 2)
  cleanup()
  target.dispatchEvent(new Event('pisper:sessions-updated'))
  await tick()
  assert.equal(revision, 2)
  unsubscribes.forEach((unsubscribe) => unsubscribe())
})

test('session mutation invalidates a pending old snapshot before the next explicit refresh', async () => {
  const old = deferred()
  let reads = 0
  globalThis.fetch = async (path, options) => {
    if (options.method === 'PUT') return json({ model: 'new' })
    reads += 1
    if (reads === 1) return old.promise
    return json({ sessions: [{ id: 's', model: 'new' }] })
  }
  const stale = chatApi.listSessions()
  await tick()
  await chatApi.updateModel('s', 'provider', 'new')
  const currentRequest = chatApi.listSessions()
  old.resolve(json({ sessions: [{ id: 's', model: 'old' }] }))
  const current = await currentRequest
  assert.equal((await stale).sessions[0].model, 'new')
  await tick()
  assert.equal(current.sessions[0].model, 'new')
  assert.equal(queryClient.getQueryData(['sessions']).sessions[0].model, 'new')
})

test('an event burst shares the pending request and publishes only the replacement snapshot', async () => {
  const old = deferred()
  let calls = 0
  globalThis.fetch = async () => {
    calls += 1
    return calls === 1 ? old.promise : json({ sessions: [{ id: 'new' }] })
  }
  const target = new EventTarget()
  const cleanup = installStartupQueryEvents(target)
  const observer = new QueryObserver(queryClient, startupQueryOptions('sessions'))
  const published = []
  const unsubscribe = observer.subscribe((result) => {
    if (result.data) published.push(result.data.sessions[0]?.id)
  })
  const read = chatApi.listSessions()
  await tick()
  for (let index = 0; index < 20; index += 1) {
    target.dispatchEvent(new Event('pisper:sessions-updated'))
  }
  await tick()
  assert.equal(calls, 1)
  old.resolve(json({ sessions: [{ id: 'old' }] }))
  assert.equal((await read).sessions[0].id, 'new')
  assert.equal(calls, 2)
  assert.ok(published.includes('new'))
  assert.ok(!published.includes('old'))
  unsubscribe()
  cleanup()
})

test('failed mutation leaves the last valid snapshot intact and invalidated config reloads', async () => {
  queryClient.setQueryData(['sessions'], { sessions: [{ id: 's' }] })
  globalThis.fetch = async () => new Response('{"error":"failed"}', { status: 500 })
  await assert.rejects(chatApi.renameSession('s', 'new'), /failed/)
  assert.equal(queryClient.getQueryState(['sessions']).isInvalidated, false)
  queryClient.setQueryData(['config'], { model: 'old' })
  await invalidateStartupQuery('config')
  globalThis.fetch = async () => json({ providers: [], model: 'new' })
  assert.equal((await chatApi.getConfig()).model, 'new')
})

test('startup store guards preserve explicit local and remote profile refresh', async () => {
  let calls = 0
  let profile = 'mobile-embedded'
  globalThis.fetch = async (path) => {
    calls += 1
    return json(path === '/api/client-info' ? { client: 'mobile-app' } : { profile })
  }
  const client = useClientStore.getState()
  await Promise.all([client.load(), client.load()])
  await client.load({ refresh: false })
  assert.equal(calls, 1)
  await client.load()
  assert.equal(calls, 2)
  const capabilities = useRuntimeCapabilitiesStore.getState()
  await Promise.all([capabilities.load(), capabilities.load()])
  await capabilities.load({ refresh: false })
  assert.equal(calls, 3)
  assert.equal(useRuntimeCapabilitiesStore.getState().capabilities.profile, 'mobile-embedded')
  profile = 'mobile-store'
  await capabilities.load()
  assert.equal(calls, 4)
  assert.equal(useRuntimeCapabilitiesStore.getState().capabilities.profile, 'mobile-store')
  profile = 'desktop'
  await capabilities.load()
  assert.equal(useRuntimeCapabilitiesStore.getState().capabilities.profile, 'desktop')
})

test('failed startup config remains an error rather than a successful empty provider list', async () => {
  globalThis.fetch = async () => new Response('{"error":"offline"}', { status: 503 })
  const observer = new QueryObserver(queryClient, {
    ...startupQueryOptions('config'),
    retry: false,
  })
  const result = await observer.refetch()
  assert.equal(result.isPending, false)
  assert.equal(result.isSuccess, false)
  assert.equal(result.data, undefined)
  globalThis.fetch = async () => json({ providers: [], model: 'restored' })
  assert.equal((await chatApi.getConfig({ refresh: true })).model, 'restored')
  observer.destroy()
})

test('store fallback can be explicitly retried after an unavailable runtime', async () => {
  globalThis.fetch = async () => {
    throw new Error('offline')
  }
  await Promise.all([
    useClientStore.getState().load(),
    useRuntimeCapabilitiesStore.getState().load(),
  ])
  assert.equal(useClientStore.getState().loaded, true)
  assert.equal(useRuntimeCapabilitiesStore.getState().loaded, true)
  globalThis.fetch = async (path) =>
    json(path === '/api/client-info' ? { client: 'mobile-app' } : { profile: 'mobile-store' })
  await Promise.all([
    useClientStore.getState().load(),
    useRuntimeCapabilitiesStore.getState().load(),
  ])
  assert.equal(useClientStore.getState().client, 'mobile-app')
  assert.equal(useRuntimeCapabilitiesStore.getState().capabilities.profile, 'mobile-store')
})

test('diagnostics are opt-in and each phase is recorded once', (context) => {
  const debug = context.mock.method(console, 'debug', () => {})
  const mark = context.mock.method(performance, 'mark', () => {})
  let enabled = false
  globalThis.window = { localStorage: { getItem: () => (enabled ? '1' : null) } }
  markStartupPhase('react-app-mounted')
  assert.equal(debug.mock.callCount(), 0)
  assert.equal(mark.mock.callCount(), 0)
  enabled = true
  markStartupPhase('react-app-mounted')
  markStartupPhase('react-app-mounted')
  assert.equal(debug.mock.callCount(), 1)
  assert.equal(mark.mock.callCount(), 1)
})
