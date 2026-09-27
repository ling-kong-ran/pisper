import assert from 'node:assert/strict'
import test from 'node:test'
import { setImmediate as tick } from 'node:timers/promises'
import { createRuntimeSelectionQueue } from '../../src/features/chat/runtime-selection-queue.ts'

function deferred() {
  let resolve, reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

test('active runs are read-only; the latest selection applies exactly once after idle', async () => {
  const probes = [deferred(), deferred()]
  let probeIndex = 0
  const writes = []
  const events = []
  const queue = createRuntimeSelectionQueue({
    intervalMs: 0,
    isStreaming: () => probes[probeIndex++].promise,
    apply: async (id, selection) => {
      writes.push({ id, selection })
      return { thinkingLevel: selection.thinkingLevel }
    },
  })
  try {
    queue.subscribe((id, entry) => events.push([id, entry?.status]))
    queue.select('a', { thinkingLevel: 'low' })
    probes[0].resolve(true)
    await tick()
    assert.equal(writes.length, 0)
    queue.select('a', { thinkingLevel: 'high' })
    probes[1].resolve(false)
    // Resolve completion from the queue, not an arbitrary timing allowance.
    await new Promise((done) => {
      const off = queue.subscribe((_id, entry) => {
        if (entry?.status === 'success') {
          queueMicrotask(off)
          done()
        }
      })
    })
    assert.deepEqual(writes, [{ id: 'a', selection: { thinkingLevel: 'high' } }])
    assert.deepEqual(
      events.filter(([, status]) => status === 'applying'),
      [['a', 'applying']],
    )
  } finally {
    await queue.dispose()
  }
})

test('leaving a route does not cancel a command; remount receives its outcome once', async () => {
  const probe = deferred()
  const applied = deferred()
  const queue = createRuntimeSelectionQueue({
    isStreaming: () => probe.promise,
    apply: async () => {
      applied.resolve()
      return { thinkingLevel: 'low' }
    },
  })
  try {
    const off = queue.subscribe(() => {})
    queue.select('a', { thinkingLevel: 'low' })
    off()
    probe.resolve(false)
    await applied.promise
    await tick()
    const entries = []
    queue.subscribe((id, entry) => entries.push([id, entry]))()
    assert.equal(entries.length, 1)
    assert.equal(entries[0][1].status, 'success')
    assert.deepEqual(entries[0][1].result, { thinkingLevel: 'low' })
    queue.subscribe(() => assert.fail('result delivered twice'))()
  } finally {
    await queue.dispose()
  }
})

test('cancel/reselect aborts stale probes without crossing session boundaries', async () => {
  const probes = []
  const writes = []
  const queue = createRuntimeSelectionQueue({
    isStreaming: (id, signal) => {
      const probe = deferred()
      probes.push({ id, signal, ...probe })
      return probe.promise
    },
    apply: async (id, selection) => {
      writes.push([id, selection.thinkingLevel])
      return {}
    },
  })
  try {
    queue.select('a', { thinkingLevel: 'low' })
    queue.select('b', { thinkingLevel: 'medium' })
    queue.select('a')
    assert.equal(probes[0].signal.aborted, true)
    assert.equal(probes[1].signal.aborted, false)
    queue.select('a', { thinkingLevel: 'high' })
    for (const probe of probes) probe.resolve(false)
    await tick()
    assert.deepEqual(writes.sort(), [
      ['a', 'high'],
      ['b', 'medium'],
    ])
  } finally {
    await queue.dispose()
  }
})

test('failed mutations surface once without automatic write retries', async () => {
  let writes = 0
  const queue = createRuntimeSelectionQueue({
    isStreaming: async () => false,
    apply: async () => {
      writes++
      throw new Error('save failed')
    },
  })
  try {
    queue.select('a', { thinkingLevel: 'high' })
    await tick()
    const events = []
    queue.subscribe((_id, entry) => events.push(entry))()
    assert.equal(writes, 1)
    assert.equal(events[0].status, 'error')
    assert.equal(events[0].error.message, 'save failed')
    await tick()
    assert.equal(writes, 1)
  } finally {
    await queue.dispose()
  }
})

test('selection cannot be replaced or cancelled during an in-flight save', async () => {
  const save = deferred()
  const queue = createRuntimeSelectionQueue({
    isStreaming: async () => false,
    apply: () => save.promise,
  })
  try {
    queue.select('a', { thinkingLevel: 'high' })
    await tick()
    assert.equal(queue.select('a', { thinkingLevel: 'low' }), false)
    assert.equal(queue.select('a'), false)
    save.resolve({ thinkingLevel: 'high' })
    await tick()
    queue.subscribe((_id, entry) => assert.equal(entry.result.thinkingLevel, 'high'))()
  } finally {
    await queue.dispose()
  }
})

test('dispose aborts outstanding read requests and suppresses late outcomes', async () => {
  let signal
  const queue = createRuntimeSelectionQueue({
    isStreaming: (_id, value) => {
      signal = value
      return new Promise((_resolve, reject) =>
        value.addEventListener('abort', () => reject(new Error('aborted')), { once: true }),
      )
    },
    apply: async () => assert.fail('disposed queue must not write'),
  })
  queue.select('a', { thinkingLevel: 'high' })
  await queue.dispose()
  assert.equal(signal.aborted, true)
  queue.subscribe(() => assert.fail('disposed outcome must not be delivered'))()
})
