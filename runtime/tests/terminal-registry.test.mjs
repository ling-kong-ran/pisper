import assert from 'node:assert/strict'
import test from 'node:test'
import { createTerminalRegistry } from '../../src/features/terminal/terminal-registry.ts'

function fixture({ delayed = false } = {}) {
  const events = new Map(),
    closes = [],
    creates = [],
    writes = []
  let finish
  const transport = {
    terminalCreate(options, onEvent) {
      creates.push(options)
      events.set(options.terminalId, onEvent)
      const value = {
        terminalId: options.terminalId,
        cwd: options.cwd,
        profileId: options.profileId,
      }
      return delayed
        ? new Promise((resolve) => {
            finish = () => resolve(value)
          })
        : Promise.resolve(value)
    },
    terminalWrite(id, data) {
      writes.push([id, [...data]])
      return Promise.resolve()
    },
    terminalResize() {
      return Promise.resolve()
    },
    terminalClose(id) {
      closes.push(id)
      return Promise.resolve(true)
    },
  }
  const registry = createTerminalRegistry(transport)
  return { registry, events, closes, creates, writes, finish: () => finish() }
}

test('view unsubscribe leaves its task terminal alive and retains background output', async () => {
  const f = fixture()
  const id = await f.registry.create('task-a', '/workspace/a', 'shell')
  const unsubscribe = f.registry.subscribe(() => {})
  unsubscribe()
  await f.registry.create('task-b', '/workspace/b', 'shell')
  f.events.get(id)({ type: 'output', terminalId: id, data: [65, 10] })
  assert.deepEqual(f.closes, [])
  assert.equal(f.registry.snapshot('task-a')[0].output, 'A\n')
  assert.equal(f.registry.snapshot('task-a')[0].cwd, '/workspace/a')
  assert.equal(f.registry.snapshot('task-b')[0].output, '')
  await f.registry.write(id, new Uint8Array([120]))
  assert.deepEqual(f.writes, [[id, [120]]])
  await f.registry.stop(id)
  assert.deepEqual(f.closes, [id])
  assert.equal(f.registry.snapshot('task-a')[0].status, 'exited')
  await f.registry.dispose()
  assert.equal(f.closes.length, 2)
})

test('pending creation is stopped after completion when the owning application disposes', async () => {
  const f = fixture({ delayed: true })
  const creating = f.registry.create('a', '/a', 'shell')
  const disposing = f.registry.dispose()
  f.finish()
  await Promise.allSettled([creating, disposing])
  assert.deepEqual(f.closes, [f.creates[0].terminalId])
  assert.equal(f.registry.snapshot()[0].status, 'exited')
  await assert.rejects(f.registry.create('b', '/b', 'shell'), /disposed/)
})

test('application exit immediately dispatches close for already-created terminals', async () => {
  const f = fixture()
  const id = await f.registry.create('a', '/a', 'shell')
  const closing = f.registry.dispose()
  assert.deepEqual(f.closes, [id])
  await closing
})

test('early output and exit events survive the eventual create response', async () => {
  const f = fixture({ delayed: true })
  const creating = f.registry.create('a', '/a', 'shell')
  const id = f.creates[0].terminalId
  f.events.get(id)({ type: 'output', terminalId: id, data: [228, 184] })
  f.events.get(id)({ type: 'output', terminalId: id, data: [173] })
  f.events.get(id)({ type: 'exit', terminalId: id, code: 7 })
  f.finish()
  await creating
  assert.equal(f.registry.snapshot('a')[0].output, '中')
  assert.equal(f.registry.snapshot('a')[0].status, 'exited')
  assert.equal(f.registry.snapshot('a')[0].exitCode, 7)
  await f.registry.dispose()
  assert.deepEqual(f.closes, [])
})

test('foreign events cannot corrupt an owned terminal and deleting its task does not rebind it', async () => {
  const f = fixture()
  const id = await f.registry.create('a', '/a', 'shell')
  const before = f.registry.snapshot('a')
  f.events.get(id)({ type: 'output', terminalId: 'different', data: [66] })
  assert.equal(f.registry.snapshot('a'), before)
  f.registry.orphan(['a'])
  assert.equal(f.registry.snapshot('a')[0].sessionId, 'a')
  assert.equal(f.registry.snapshot('a')[0].cwd, '/a')
  assert.equal(f.registry.snapshot('a')[0].orphaned, true)
  assert.deepEqual(f.registry.snapshot('b'), [])
  await f.registry.dispose()
})

test('failed stop remains observable and can be retried without pretending the process exited', async () => {
  let failing = true
  const registry = createTerminalRegistry({
    terminalCreate: (options) => Promise.resolve({ ...options }),
    terminalWrite: () => Promise.resolve(),
    terminalResize: () => Promise.resolve(),
    terminalClose: () => (failing ? Promise.reject(new Error('offline')) : Promise.resolve(true)),
  })
  const id = await registry.create('a', '/a', 'shell')
  await assert.rejects(registry.stop(id), /offline/)
  assert.notEqual(registry.snapshot('a')[0].status, 'exited')
  failing = false
  await registry.stop(id)
  assert.equal(registry.snapshot('a')[0].status, 'exited')
})

test('logs decode split control sequences and bound a single oversized output event', async () => {
  const f = fixture()
  const id = await f.registry.create('a', '/a', 'shell')
  const emit = (text) =>
    f.events.get(id)({ type: 'output', terminalId: id, data: [...new TextEncoder().encode(text)] })
  emit('\x1b[31')
  assert.equal(f.registry.snapshot('a')[0].output, '')
  emit('mred\x1b[0m\x1b]0;title')
  emit('\x07 plain')
  assert.equal(f.registry.snapshot('a')[0].output, 'red plain')
  f.events.get(id)({ type: 'output', terminalId: id, data: Array(2 * 1024 * 1024 + 1).fill(65) })
  assert.ok(
    f.registry.outputChunks(id).reduce((size, chunk) => size + chunk.data.length, 0) <=
      2 * 1024 * 1024,
  )
  assert.equal(f.registry.snapshot('a')[0].truncated, true)
  await f.registry.dispose()
})
