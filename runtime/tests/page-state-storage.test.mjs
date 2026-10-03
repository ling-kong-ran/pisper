import assert from 'node:assert/strict'
import test from 'node:test'
import { setImmediate } from 'node:timers/promises'

test('page state restores server values before stores load and migrates local-only preferences', async () => {
  const values = new Map([
    ['pisper-ui', 'old-theme'],
    ['pisper-shortcuts', 'local-shortcuts'],
    ['pisper-chat-layout', 'x'.repeat(20 * 64 * 1024)],
  ])
  const updates = []
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: {
      localStorage: {
        getItem: (key) => values.get(key) ?? null,
        setItem: (key, value) => values.set(key, value),
        removeItem: (key) => values.delete(key),
      },
      addEventListener() {},
      fetch: async (_path, options = {}) => {
        if (!options.method)
          return Response.json({
            version: 1,
            values: {
              'pisper-ui': 'saved-theme',
              'pisper-language': 'zh-CN',
              'pisper-chat-layout': 'x'.repeat(20 * 64 * 1024),
            },
            revisions: { 'pisper-chat-layout': 100 },
          })
        updates.push(JSON.parse(options.body).updates)
        return new Response(null, { status: 204 })
      },
    },
  })
  try {
    const { pageStateStorage, restorePageState } =
      await import('../../src/lib/storage/page-state-storage.ts')
    await restorePageState()
    assert.equal(values.get('pisper-ui'), 'saved-theme')
    assert.equal(values.get('pisper-language'), 'zh-CN')
    assert.equal(values.has('pisper-chat-layout'), false)
    assert.deepEqual(updates, [{ 'pisper-shortcuts': 'local-shortcuts' }])
    pageStateStorage.setItem('pisper-ui', 'next-theme')
    await setImmediate()
    assert.equal(values.get('pisper-ui'), 'next-theme')
    assert.deepEqual(updates.at(-1), { 'pisper-ui': 'next-theme' })
    assert.throws(() => pageStateStorage.setItem('api-key', 'secret'))
  } finally {
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})

test('refresh keeps a recent close while its Runtime write is still unconfirmed', async () => {
  const key = 'pisper-session-context-layout'
  const open = '{"state":{"open":true}}'
  const closed = '{"state":{"open":false}}'
  const local = new Map([[key, open]])
  const session = new Map()
  const storage = (values) => ({
    getItem: (name) => values.get(name) ?? null,
    setItem: (name, value) => values.set(name, value),
    removeItem: (name) => values.delete(name),
  })
  let releaseFirstWrite
  let writes = 0
  const firstWrite = new Promise((resolve) => {
    releaseFirstWrite = resolve
  })
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: {
      localStorage: storage(local),
      sessionStorage: storage(session),
      addEventListener() {},
      fetch: async (_path, options = {}) => {
        if (!options.method)
          return Response.json({ version: 1, values: { [key]: open }, revisions: { [key]: 1 } })
        writes += 1
        if (writes === 1) await firstWrite
        return new Response(null, { status: 204 })
      },
    },
  })
  try {
    const { pageStateStorage, restorePageState } =
      await import('../../src/lib/storage/page-state-storage.ts')
    await restorePageState()
    pageStateStorage.setItem(key, closed)
    assert.equal(local.get(key), closed)
    assert.equal(writes, 1)
    await restorePageState()
    assert.equal(local.get(key), closed)
    assert.ok(session.size, 'unconfirmed choice survives a reload in the same origin')
    releaseFirstWrite()
    await setImmediate()
    await setImmediate()
    assert.equal(writes, 2)
    assert.equal(session.size, 0, 'acknowledged choice leaves no stale pending marker')
  } finally {
    releaseFirstWrite()
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})

test('a stalled migration cannot block startup or permanently block newer preference writes', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const key = 'pisper-session-context-layout'
  const values = new Map([[key, 'open']])
  const writes = []
  let stalledSignal
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: {
      localStorage: {
        getItem: (name) => values.get(name) ?? null,
        setItem: (name, value) => values.set(name, value),
        removeItem: (name) => values.delete(name),
      },
      setTimeout: globalThis.setTimeout.bind(globalThis),
      addEventListener() {},
      fetch: async (_path, options = {}) => {
        if (!options.method) return Response.json({ version: 1, values: {} })
        writes.push(JSON.parse(options.body).updates)
        if (writes.length === 1) {
          stalledSignal = options.signal
          await new Promise((_, reject) => {
            options.signal.addEventListener('abort', () => reject(options.signal.reason), {
              once: true,
            })
          })
        }
        return new Response(null, { status: 204 })
      },
    },
  })
  try {
    const { pageStateStorage, restorePageState } =
      await import('../../src/lib/storage/page-state-storage.ts')
    await restorePageState()
    assert.equal(writes.length, 1, 'startup returns while the initial write is pending')
    pageStateStorage.setItem(key, 'closed')
    t.mock.timers.tick(3000)
    await setImmediate()
    assert.equal(stalledSignal.aborted, true)
    t.mock.timers.tick(2000)
    await setImmediate()
    assert.deepEqual(writes, [{ [key]: 'open' }, { [key]: 'closed' }])
    assert.equal(values.get(key), 'closed')
  } finally {
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})
