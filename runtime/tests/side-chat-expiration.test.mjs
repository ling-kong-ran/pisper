import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { setImmediate } from 'node:timers/promises'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'

const providerCode = transformSync(
  await readFile('src/features/chat/SideChatProvider.tsx', 'utf8'),
  { loader: 'tsx', format: 'cjs', jsx: 'automatic' },
).code
const BASE_TIME = Date.parse('2026-09-27T00:00:00.000Z')
const SIDE_ID = 'side-expiration-test'
const emptyEntry = {
  session: null,
  expiresAt: null,
  created: false,
  loading: false,
  error: '',
  draft: '',
  expired: false,
}
const absent = { session: null, expiresAt: null, created: false }
function response(expiresAt = BASE_TIME + 1000) {
  return {
    session: { id: SIDE_ID, streaming: true },
    expiresAt: new Date(expiresAt).toISOString(),
    created: false,
  }
}
function deferred() {
  let resolve
  let reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

// 使用真实 Provider 生命周期与关联状态，仅替换 React 提交调度、时钟与网络。
// 网络故意忽略 signal，验证卸载后晚到的响应不会重新占用会话状态。
function fixture(t) {
  const slots = []
  const requests = []
  const intervals = new Map()
  const listeners = new Set()
  const leases = []
  const clearedDrafts = []
  const discarded = []
  const syncs = []
  let cursor = 0
  let effects = []
  let dirty = true
  let mounted = true
  let rendered
  let now = BASE_TIME
  let nextTimer = 1
  const document = {
    visibilityState: 'visible',
    addEventListener(event, callback) {
      assert.equal(event, 'visibilitychange')
      listeners.add(callback)
    },
    removeEventListener(event, callback) {
      assert.equal(event, 'visibilitychange')
      listeners.delete(callback)
    },
  }
  const unchanged = (before, after) =>
    before &&
    before.length === after.length &&
    after.every((item, index) => Object.is(item, before[index]))
  const memo = (factory, dependencies) => {
    const index = cursor++
    if (!unchanged(slots[index]?.dependencies, dependencies))
      slots[index] = { value: factory(), dependencies }
    return slots[index].value
  }
  const effect = (phase) => (callback, dependencies) => {
    const index = cursor++
    if (!unchanged(slots[index]?.dependencies, dependencies))
      effects.push({ index, phase, callback, dependencies })
  }
  const request = (method) => (parentId, signal) => {
    const pending = deferred()
    requests.push({ method, parentId, signal, ...pending })
    return pending.promise
  }
  const runtime = {
    discard: (id) => discarded.push(id),
    getSessionState: () => ({ streaming: true }),
    retainSessionState(id) {
      const lease = { id, releases: 0 }
      leases.push(lease)
      return () => {
        lease.releases += 1
      }
    },
    syncLiveSession: async (id) => {
      syncs.push(id)
    },
    loadSessionMessages: async () => {},
  }
  const modules = {
    react: {
      useState(initial) {
        const index = cursor++
        slots[index] ??= { value: initial }
        return [
          slots[index].value,
          (value) => {
            slots[index].value = value
            dirty = true
          },
        ]
      },
      useRef(initial) {
        const index = cursor++
        slots[index] ??= { current: initial }
        return slots[index]
      },
      useCallback: (callback, dependencies) => memo(() => callback, dependencies),
      useMemo: memo,
      useLayoutEffect: effect('layout'),
      useEffect: effect('passive'),
    },
    'react/jsx-runtime': { jsx: (type, props) => ({ type, props }) },
    './chat-errors': { chatErrorMessage: (error) => error.message },
    './composer-drafts': { clearComposerDraft: (id) => clearedDrafts.push(id) },
    './side-chat-api': { getSideChat: request('GET'), ensureSideChat: request('POST') },
    './side-chat-context': { SideChatContext: { Provider: 'provider' }, EMPTY_ENTRY: emptyEntry },
  }
  const module = { exports: {} }
  runInNewContext(providerCode, {
    module,
    exports: module.exports,
    AbortController,
    document,
    Date: class ClockDate extends Date {
      static now() {
        return now
      }
    },
    window: {
      setInterval(callback, delay) {
        const id = nextTimer++
        intervals.set(id, { callback, delay })
        return id
      },
      clearInterval(id) {
        intervals.delete(id)
      },
    },
    require(id) {
      assert.ok(Object.hasOwn(modules, id), id)
      return modules[id]
    },
  })
  const render = () => {
    assert.equal(mounted, true)
    cursor = 0
    effects = []
    dirty = false
    rendered = module.exports.SideChatProvider({ runtime, children: null })
    for (const phase of ['layout', 'passive']) {
      for (const pending of effects.filter((item) => item.phase === phase)) {
        slots[pending.index]?.cleanup?.()
        slots[pending.index] = { dependencies: pending.dependencies, cleanup: pending.callback() }
      }
    }
    return rendered.props.value
  }
  const unmount = () => {
    if (!mounted) return
    mounted = false
    for (const slot of slots) slot?.cleanup?.()
  }
  t.after(unmount)
  render()
  return {
    requests,
    intervals,
    listeners,
    leases,
    clearedDrafts,
    discarded,
    syncs,
    runtime,
    unmount,
    get context() {
      return dirty ? render() : rendered.props.value
    },
    advance(milliseconds) {
      now += milliseconds
    },
    tick() {
      for (const timer of [...intervals.values()]) timer.callback()
    },
    visibility(value) {
      document.visibilityState = value
      for (const listener of [...listeners]) listener()
    },
  }
}
async function open(f) {
  const pending = f.context.load('parent')
  f.requests.at(-1).resolve(response())
  await pending
  f.context.setDraft('parent', 'keep this draft')
  assert.equal(f.context.entries.parent.session.id, SIDE_ID)
}

for (const trigger of ['timer', 'visible']) {
  test(`expired side chat uses authoritative GET on ${trigger} even when local streaming is stale`, async (t) => {
    const f = fixture(t)
    await open(f)
    assert.equal(f.runtime.getSessionState(SIDE_ID).streaming, true)
    f.advance(1001)
    if (trigger === 'timer') f.tick()
    else {
      f.visibility('hidden')
      assert.equal(f.requests.length, 1, 'hidden-page events must not inspect expiry')
      f.visibility('visible')
    }
    assert.equal(f.requests.length, 2)
    assert.equal(f.requests[1].method, 'GET')
    assert.equal(f.requests[1].parentId, 'parent')
    f.tick()
    f.visibility('visible')
    assert.equal(f.requests.length, 2, 'concurrent expiration checks reuse the active request')
    f.requests[1].resolve(absent)
    await setImmediate()
    assert.equal(f.context.entries.parent.session, null)
    assert.equal(f.context.entries.parent.expired, true)
    assert.equal(f.context.entries.parent.draft, 'keep this draft')
    assert.equal(f.leases[0].releases, 1)
    assert.deepEqual(f.clearedDrafts, [SIDE_ID])
    assert.deepEqual(f.discarded, [SIDE_ID])
    f.tick()
    assert.equal(f.requests.length, 2, 'expired UI waits for an explicit new-chat action')
  })
}

test('a server-protected running side chat stays open despite its old expiry timestamp', async (t) => {
  const f = fixture(t)
  await open(f)
  f.advance(1001)
  f.tick()
  f.requests[1].resolve(response())
  await setImmediate()
  assert.equal(f.context.entries.parent.session.id, SIDE_ID)
  assert.equal(f.context.entries.parent.expired, false)
  assert.equal(f.context.entries.parent.draft, 'keep this draft')
  assert.equal(f.leases.length, 1)
  assert.equal(f.leases[0].releases, 0)
  assert.deepEqual(f.discarded, [])
  assert.ok(f.requests.every((request) => request.method === 'GET'))
})

test('unmount cancels expiry timers and requests, releases state, and ignores late metadata', async (t) => {
  const f = fixture(t)
  await open(f)
  f.advance(1001)
  f.tick()
  const pending = f.requests[1]
  const actions = f.context
  const syncCount = f.syncs.length
  assert.equal(f.intervals.size, 1)
  assert.equal(f.listeners.size, 1)
  f.unmount()
  assert.equal(pending.signal.aborted, true)
  assert.equal(f.intervals.size, 0)
  assert.equal(f.listeners.size, 0)
  assert.equal(f.leases[0].releases, 1)
  assert.deepEqual(f.discarded, [], 'route unmount must preserve the background session')
  pending.resolve({ ...response(BASE_TIME + 86400000), session: { id: 'side-late' } })
  await setImmediate()
  assert.equal(f.syncs.length, syncCount)
  assert.equal(f.leases.length, 1)
  assert.equal(await actions.load('parent', true), null)
  assert.equal(f.requests.length, 2)
})

test('a replacement ID discards only the old side chat and keeps its draft', async (t) => {
  const f = fixture(t)
  await open(f)
  const refreshing = f.context.load('parent')
  f.requests[1].resolve({ ...response(BASE_TIME + 86400000), session: { id: 'side-replacement' } })
  await refreshing
  assert.deepEqual(f.discarded, [SIDE_ID])
  assert.equal(f.context.entries.parent.session.id, 'side-replacement')
  assert.equal(f.context.entries.parent.expired, false)
  assert.equal(f.context.entries.parent.draft, 'keep this draft')
  assert.equal(f.leases[0].releases, 1)
  assert.equal(f.leases[1].id, 'side-replacement')
})
