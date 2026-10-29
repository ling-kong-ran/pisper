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

test('graphite is the default for new installs while persisted accents keep their v3 migration', async () => {
  const values = new Map()
  const previousStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  }
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: storage })
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { localStorage: storage },
  })
  try {
    const { DEFAULT_UI_PREFERENCES, useUiStore: store } =
      await import('../../src/stores/ui-store.ts')
    // 新安装（没有任何持久化值）与「恢复默认外观」都落在石墨。
    assert.equal(DEFAULT_UI_PREFERENCES.accent, 'neutral')
    assert.equal(store.getInitialState().accent, 'neutral')

    // 旧数据仍按 v3 迁移：当时无法区分默认与主动选择，旧中性档和缺省都迁到 blue；
    // 已是 v3 的数据（含主动选择的石墨）原样保留，不因默认值变化被改写。
    for (const [version, accent, expected] of [
      [2, 'neutral', 'blue'],
      [2, undefined, 'blue'],
      [2, 'teal', 'teal'],
      [3, 'neutral', 'neutral'],
      [3, 'blue', 'blue'],
      [3, 'custom', 'custom'],
    ]) {
      values.set('pisper-ui', JSON.stringify({ version, state: { theme: 'light', accent } }))
      await store.persist.rehydrate()
      assert.equal(store.getState().accent, expected, `v${version} ${accent}`)
      assert.equal(JSON.parse(values.get('pisper-ui')).version, 3)
    }

    store.getState().resetAppearance()
    assert.equal(store.getState().accent, 'neutral')
  } finally {
    if (previousStorage) Object.defineProperty(globalThis, 'localStorage', previousStorage)
    else delete globalThis.localStorage
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else delete globalThis.window
  }
})
