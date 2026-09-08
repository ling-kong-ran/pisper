import assert from 'node:assert/strict'
import { getEventListeners } from 'node:events'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import {
  abortReason,
  createAbortScope,
  throwIfAborted,
  waitWithAbort,
} from '../../src/lib/abort-signal.ts'

test('aborting a readiness wait settles immediately without cancelling shared recovery', async () => {
  let finish
  const work = new Promise((resolve) => (finish = resolve))
  const cancelled = new AbortController()
  const other = new AbortController()
  const first = waitWithAbort(work, cancelled.signal)
  const second = waitWithAbort(work, other.signal)
  cancelled.abort(new Error('request deadline'))
  await assert.rejects(first, /request deadline/)
  assert.equal(getEventListeners(cancelled.signal, 'abort').length, 0)
  finish('ready')
  assert.equal(await second, 'ready')
  assert.equal(getEventListeners(other.signal, 'abort').length, 0)
})

test('late readiness rejection is consumed after a pre-aborted wait', async () => {
  let fail
  const work = new Promise((_resolve, reject) => (fail = reject))
  const controller = new AbortController()
  controller.abort()
  await assert.rejects(waitWithAbort(work, controller.signal), { name: 'AbortError' })
  fail(new Error('late native failure'))
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(getEventListeners(controller.signal, 'abort').length, 0)
})

test('abort scopes forward cancellation and release listeners on completion or abort', () => {
  const parent = new AbortController()
  const scope = createAbortScope(parent.signal, 60000)
  assert.equal(getEventListeners(parent.signal, 'abort').length, 1)
  const reason = new Error('cancelled by owner')
  parent.abort(reason)
  assert.equal(scope.signal.aborted, true)
  assert.equal(abortReason(scope.signal), reason)
  assert.throws(
    () => throwIfAborted(scope.signal),
    (error) => error === reason,
  )
  assert.equal(getEventListeners(parent.signal, 'abort').length, 0)
  scope.dispose()
  const next = new AbortController()
  const completed = createAbortScope(next.signal, 60000)
  completed.dispose()
  assert.equal(getEventListeners(next.signal, 'abort').length, 0)
  next.abort()
  assert.equal(completed.signal.aborted, false)
})

test('pre-abort and timeout require no static AbortSignal methods', async (t) => {
  const parent = new AbortController()
  parent.abort(new Error('before initialization'))
  const before = createAbortScope(parent.signal, 60000)
  t.after(before.dispose)
  assert.equal(before.signal.aborted, true)
  assert.equal(getEventListeners(parent.signal, 'abort').length, 0)
  const timed = createAbortScope(undefined, 5)
  t.after(timed.dispose)
  await new Promise((resolve) => timed.signal.addEventListener('abort', resolve, { once: true }))
  assert.throws(() => throwIfAborted(timed.signal), { name: 'TimeoutError' })
  assert.doesNotThrow(() => throwIfAborted())
})

test('legacy system signals without reason or throwIfAborted still reject and clean ownership', async () => {
  class LegacyAbortController {
    signal = Object.assign(new EventTarget(), { aborted: false })
    abort() {
      if (this.signal.aborted) return
      this.signal.aborted = true
      this.signal.dispatchEvent(new Event('abort'))
    }
  }
  const code = transformSync(await readFile('src/lib/abort-signal.ts', 'utf8'), {
    loader: 'ts',
    format: 'cjs',
  }).code
  const module = { exports: {} }
  runInNewContext(code, {
    module,
    exports: module.exports,
    AbortController: LegacyAbortController,
    DOMException,
    setTimeout,
    clearTimeout,
  })
  const parent = new LegacyAbortController()
  const scope = module.exports.createAbortScope(parent.signal, 60000)
  parent.abort()
  assert.equal(scope.signal.aborted, true)
  assert.throws(() => module.exports.throwIfAborted(scope.signal), { name: 'AbortError' })
  assert.equal(getEventListeners(parent.signal, 'abort').length, 0)
  scope.dispose()
})
