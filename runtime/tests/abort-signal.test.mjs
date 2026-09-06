import assert from 'node:assert/strict'
import { getEventListeners } from 'node:events'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import { abortReason, createAbortScope, throwIfAborted } from '../../src/lib/abort-signal.ts'

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
