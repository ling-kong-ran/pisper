import assert from 'node:assert/strict'
import test from 'node:test'

const STORAGE_KEY = 'pisper-session-context-layout'

test('context width preferences remain usable across reloads and storage failures', async (t) => {
  const values = new Map([[STORAGE_KEY, JSON.stringify({ state: { width: 515.6 }, version: 1 })]])
  let writes = 0
  let readFails = false
  let writeFails = false
  const storage = {
    getItem: (key) => {
      if (readFails) throw new Error('Storage unavailable')
      return values.get(key) ?? null
    },
    setItem: (key, value) => {
      if (writeFails) throw new Error('Storage quota exceeded')
      writes += 1
      values.set(key, value)
    },
    removeItem: (key) => {
      if (writeFails) throw new Error('Storage unavailable')
      values.delete(key)
    },
  }
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { localStorage: storage },
  })

  try {
    const { useSessionContextStore: store } =
      await import('../../src/features/chat/session-context-store.ts')

    await t.test('restores the saved width without rewriting storage on mount', () => {
      assert.equal(store.getState().width, 516)
      assert.equal(writes, 0)
    })

    await t.test('saves only width and restores it after a reload', async () => {
      store.getState().setWidth(600)
      const saved = values.get(STORAGE_KEY)
      assert.deepEqual(JSON.parse(saved), { state: { width: 600 }, version: 1 })
      const writesBeforeRepeat = writes
      store.getState().setWidth(600.2)
      assert.equal(writes, writesBeforeRepeat)
      store.setState({ width: 360 })
      values.set(STORAGE_KEY, saved)
      await store.persist.rehydrate()
      assert.equal(store.getState().width, 600)
    })

    await t.test('normalizes malformed stored shapes and width values', async () => {
      for (const state of [null, [], 480, 'wide', {}, { width: '600' }, { width: null }]) {
        values.set(STORAGE_KEY, JSON.stringify({ state, version: 1 }))
        await store.persist.rehydrate()
        assert.equal(store.getState().width, 360)
        assert.equal(typeof store.getState().setWidth, 'function')
      }
      for (const [width, expected] of [
        [-1, 280],
        [10000, 720],
      ]) {
        values.set(STORAGE_KEY, JSON.stringify({ state: { width }, version: 1 }))
        await store.persist.rehydrate()
        assert.equal(store.getState().width, expected)
      }
    })

    await t.test('failed storage reads leave an adjustable default layout', async () => {
      readFails = true
      await assert.doesNotReject(store.persist.rehydrate())
      assert.equal(store.getState().width, 360)
      readFails = false
      store.getState().setWidth(480)
      assert.equal(store.getState().width, 480)
    })

    await t.test('corrupted JSON cannot prevent later width changes', async () => {
      values.set(STORAGE_KEY, '{invalid JSON')
      await assert.doesNotReject(store.persist.rehydrate())
      assert.doesNotThrow(() => store.getState().setWidth(500))
      assert.equal(store.getState().width, 500)
      assert.equal(JSON.parse(values.get(STORAGE_KEY)).state.width, 500)
    })

    await t.test('write and removal failures keep the in-memory width without throwing', () => {
      const saved = values.get(STORAGE_KEY)
      writeFails = true
      assert.doesNotThrow(() => store.getState().setWidth(620))
      assert.equal(store.getState().width, 620)
      assert.equal(values.get(STORAGE_KEY), saved)
      assert.doesNotThrow(() => store.persist.clearStorage())
      writeFails = false
      store.getState().setWidth(640)
      assert.equal(JSON.parse(values.get(STORAGE_KEY)).state.width, 640)
    })

    await t.test('denied localStorage access cannot stop layout adjustment', () => {
      Object.defineProperty(globalThis, 'window', {
        configurable: true,
        value: {
          get localStorage() {
            throw new Error('Access denied')
          },
        },
      })
      assert.doesNotThrow(() => store.getState().setWidth(560))
      assert.equal(store.getState().width, 560)
    })
  } finally {
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})
