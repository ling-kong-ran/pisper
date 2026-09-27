import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { setImmediate } from 'node:timers/promises'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import * as sessionContextLayout from '../../src/features/chat/session-context-layout.ts'

const hookCode = transformSync(
  await readFile('src/features/chat/useSessionContextAutoReveal.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code
const RUN_STARTED_AT = '2026-09-27T10:00:00.000Z'

function fileChange(overrides = {}) {
  return {
    path: 'example.txt',
    status: 'modified',
    added: 1,
    removed: 0,
    changeCount: 1,
    snapshot: true,
    canRevert: true,
    approved: false,
    reverted: false,
    pending: true,
    changedAt: '2026-09-27T10:00:01.000Z',
    ...overrides,
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

// 保留真实 hook 和文件判定，只替换 React 的提交调度与网络响应。
// 请求故意不监听 abort，确保旧响应即使晚到也不能重新打开面板。
function fixture(t) {
  const slots = []
  const requests = []
  const reveals = []
  let cursor = 0
  let effects = []
  let mounted = true
  let options = {
    sessionId: 'session',
    streaming: false,
    completed: false,
    runStartedAt: RUN_STARTED_AT,
    enabled: true,
    open: false,
    onReveal: () => reveals.push('revealed'),
  }
  const registerEffect = (phase) => (callback, dependencies) => {
    const index = cursor++
    const previous = slots[index]
    if (
      previous &&
      dependencies.length === previous.dependencies.length &&
      dependencies.every((dependency, offset) =>
        Object.is(dependency, previous.dependencies[offset]),
      )
    )
      return
    effects.push({ phase, index, callback, dependencies })
  }
  const modules = {
    react: {
      useRef(initial) {
        const index = cursor++
        slots[index] ??= { current: initial }
        return slots[index]
      },
      useLayoutEffect: registerEffect('layout'),
      useEffect: registerEffect('passive'),
    },
    './chat-api': {
      chatApi: {
        getSessionFileChanges(sessionId, { signal }) {
          const pending = deferred()
          requests.push({ sessionId, signal, ...pending })
          return pending.promise
        },
      },
    },
    './session-context-layout': sessionContextLayout,
  }
  const module = { exports: {} }
  runInNewContext(hookCode, {
    module,
    exports: module.exports,
    AbortController,
    require(id) {
      assert.ok(Object.hasOwn(modules, id), id)
      return modules[id]
    },
  })
  const unmount = () => {
    if (!mounted) return
    mounted = false
    for (const slot of slots) slot?.cleanup?.()
  }
  t.after(unmount)
  return {
    requests,
    reveals,
    unmount,
    render(patch = {}) {
      assert.equal(mounted, true, 'cannot render an unmounted fixture')
      options = { ...options, ...patch }
      cursor = 0
      effects = []
      module.exports.useSessionContextAutoReveal(options)
      for (const phase of ['layout', 'passive']) {
        for (const effect of effects.filter((item) => item.phase === phase)) {
          slots[effect.index]?.cleanup?.()
          slots[effect.index] = {
            dependencies: effect.dependencies,
            cleanup: effect.callback(),
          }
        }
      }
    },
  }
}

test('context inspection runs only when an observed active run finishes successfully', (t) => {
  const f = fixture(t)
  f.render({ completed: true })
  f.render()
  assert.equal(f.requests.length, 0, 'loading a completed history must not inspect files')
  f.render({ streaming: true, completed: false })
  f.render()
  assert.equal(f.requests.length, 0, 'streaming alone must not inspect files')
  f.render({ streaming: false, completed: false })
  assert.equal(f.requests.length, 0, 'stopped or failed runs must not inspect files')
  f.render({ streaming: true })
  f.render({ streaming: false, completed: true })
  f.render()
  assert.equal(f.requests.length, 1, 'one successful transition produces exactly one request')
  assert.equal(f.requests[0].sessionId, 'session')
  assert.equal(f.requests[0].signal.aborted, false)
})

for (const [scenario, files] of [
  ['empty snapshots', []],
  ['historical file changes', [fileChange({ changedAt: '2026-09-26T10:00:01.000Z' })]],
]) {
  test(`${scenario} do not open context after a successful chat response`, async (t) => {
    const f = fixture(t)
    f.render({ streaming: true })
    f.render({ streaming: false, completed: true })
    f.requests[0].resolve({ files })
    await setImmediate()
    assert.deepEqual(f.reveals, [])
    assert.equal(f.requests.length, 1)
  })
}

test('current-run changes reveal context with the latest callback without restarting inspection', async (t) => {
  const f = fixture(t)
  const latestReveals = []
  f.render({ streaming: true })
  f.render({ streaming: false, completed: true })
  f.render({ onReveal: () => latestReveals.push('latest') })
  assert.equal(f.requests.length, 1)
  assert.equal(f.requests[0].signal.aborted, false)
  f.requests[0].resolve({ files: [fileChange()] })
  await setImmediate()
  assert.deepEqual(f.reveals, [])
  assert.deepEqual(latestReveals, ['latest'])
  f.render()
  assert.equal(f.requests.length, 1)
})

for (const [scenario, state, reset] of [
  ['an already open panel', { open: true }, { open: false }],
  ['disabled automatic context', { enabled: false }, { enabled: true }],
]) {
  test(`${scenario} does not inspect files or reveal a completed run retroactively`, (t) => {
    const f = fixture(t)
    f.render({ streaming: true, ...state })
    f.render({ streaming: false, completed: true })
    f.render(reset)
    assert.equal(f.requests.length, 0)
    assert.deepEqual(f.reveals, [])
  })
}

for (const [scenario, state] of [
  ['switching sessions', { sessionId: 'other-session' }],
  [
    'starting a new run',
    { streaming: true, completed: false, runStartedAt: '2026-09-27T10:01:00.000Z' },
  ],
  ['opening context manually', { open: true }],
  ['disabling automatic context', { enabled: false }],
  ['unmounting the chat', null],
]) {
  test(`${scenario} cancels inspection and ignores a late response`, async (t) => {
    const f = fixture(t)
    f.render({ streaming: true })
    f.render({ streaming: false, completed: true })
    const pending = f.requests[0]
    if (state) f.render(state)
    else f.unmount()
    assert.equal(pending.signal.aborted, true)
    pending.resolve({ files: [fileChange()] })
    await setImmediate()
    assert.deepEqual(f.reveals, [])
    assert.equal(f.requests.length, 1)
  })
}

test('a failed inspection keeps context closed and the next successful run can inspect again', async (t) => {
  const f = fixture(t)
  f.render({ streaming: true })
  f.render({ streaming: false, completed: true })
  f.requests[0].reject(new Error('file snapshots unavailable'))
  await setImmediate()
  assert.deepEqual(f.reveals, [])
  f.render()
  assert.equal(f.requests.length, 1, 'failed inspection is not retried for the same completion')
  f.render({
    streaming: true,
    completed: false,
    runStartedAt: '2026-09-27T10:01:00.000Z',
  })
  f.render({ streaming: false, completed: true })
  assert.equal(f.requests.length, 2)
  f.requests[1].resolve({ files: [fileChange({ changedAt: '2026-09-27T10:01:01.000Z' })] })
  await setImmediate()
  assert.deepEqual(f.reveals, ['revealed'])
})

test('an old run response cannot open context while the next run awaits its own snapshot', async (t) => {
  const f = fixture(t)
  f.render({ streaming: true })
  f.render({ streaming: false, completed: true })
  f.render({
    streaming: true,
    completed: false,
    runStartedAt: '2026-09-27T10:01:00.000Z',
  })
  f.render({ streaming: false, completed: true })
  assert.equal(f.requests.length, 2)
  assert.equal(f.requests[0].signal.aborted, true)
  assert.equal(f.requests[1].signal.aborted, false)
  f.requests[0].resolve({ files: [fileChange()] })
  await setImmediate()
  assert.deepEqual(f.reveals, [])
  f.requests[1].resolve({ files: [fileChange({ changedAt: '2026-09-27T10:01:01.000Z' })] })
  await setImmediate()
  assert.deepEqual(f.reveals, ['revealed'])
})
