import assert from 'node:assert/strict'
import test from 'node:test'
import { resolveDarkTheme } from '../../src/app/ui-preferences.ts'

test('system theme follows OS preference while explicit themes ignore OS and clock', () => {
  for (const hour of [0, 7, 8, 12, 18, 23]) {
    for (const systemDark of [true, false]) {
      assert.equal(resolveDarkTheme('system', systemDark, hour), systemDark)
      assert.equal(resolveDarkTheme('dark', systemDark, hour), true)
      assert.equal(resolveDarkTheme('light', systemDark, hour), false)
      assert.equal(resolveDarkTheme('scheduled', systemDark, hour), hour < 8 || hour >= 18)
    }
  }
})

test('theme cycling persists three modes without discarding legacy scheduled preferences', async () => {
  const values = new Map()
  const previousStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  }
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: storage,
  })
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { localStorage: storage },
  })
  try {
    const { useUiStore: store } = await import('../../src/stores/ui-store.ts')
    assert.equal(store.getState().theme, 'system')
    for (const expected of ['dark', 'light', 'system', 'dark', 'light', 'system']) {
      store.getState().cycleTheme()
      assert.equal(store.getState().theme, expected)
      assert.equal(JSON.parse(values.get('pisper-ui')).state.theme, expected)
      await store.persist.rehydrate()
      assert.equal(store.getState().theme, expected)
    }

    for (const [version, theme] of [
      [0, 'system'],
      [1, 'scheduled'],
    ]) {
      values.set('pisper-ui', JSON.stringify({ version, state: { theme } }))
      await store.persist.rehydrate()
      assert.equal(store.getState().theme, 'scheduled')
      store.getState().cycleTheme()
      assert.equal(store.getState().theme, 'system')
    }
    for (const legacy of ['system', 'scheduled']) {
      store.getState().setTheme('light')
      values.delete('pisper-ui')
      values.set('pisper-theme', legacy)
      await store.persist.rehydrate()
      assert.equal(store.getState().theme, 'scheduled')
      store.getState().cycleTheme()
      assert.equal(store.getState().theme, 'system')
    }
  } finally {
    if (previousStorage) Object.defineProperty(globalThis, 'localStorage', previousStorage)
    else delete globalThis.localStorage
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else delete globalThis.window
  }
})
