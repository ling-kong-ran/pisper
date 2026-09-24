import assert from 'node:assert/strict'
import test from 'node:test'

const STORAGE_KEY = 'pisper-floating-widgets'

test('floating widget preferences preserve explicit choices and validate persisted data', async (t) => {
  const values = new Map()
  let readsDenied = false
  let writesDenied = false
  let writes = 0
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: {
      localStorage: {
        getItem(key) {
          if (readsDenied) throw new Error('Storage denied')
          return values.get(key) ?? null
        },
        setItem(key, value) {
          if (writesDenied) throw new Error('Storage denied')
          writes += 1
          values.set(key, value)
        },
        removeItem(key) {
          if (writesDenied) throw new Error('Storage denied')
          values.delete(key)
        },
      },
    },
  })
  try {
    const { useFloatingWidgetsStore: store, resolveFloatingWidgetIds } =
      await import('../../src/features/custom-ui/floating-widgets-store.ts')
    const seed = async (state, version = 1) => {
      values.set(STORAGE_KEY, JSON.stringify({ state, version }))
      await store.persist.rehydrate()
    }

    await t.test('startup is empty and does not write or materialize template defaults', () => {
      assert.deepEqual(store.getState().prefs, {})
      assert.equal(store.getState().storageError, false)
      assert.equal(writes, 0)
      assert.deepEqual(resolveFloatingWidgetIds(['pisper-island'], store.getState().prefs), [
        'pisper-island',
      ])
      assert.equal(writes, 0)
      assert.deepEqual(store.getState().prefs, {})
    })

    await t.test(
      'explicit false overrides defaults and true adds unique components in stable order',
      () => {
        const defaults = ['pisper-island', 'weather', 'weather']
        const prefs = { 'pisper-island': false, weather: true, timer: true, removed: false }
        assert.deepEqual(resolveFloatingWidgetIds(defaults, prefs), ['weather', 'timer'])
        assert.deepEqual(resolveFloatingWidgetIds(['timer', 'weather'], prefs), [
          'timer',
          'weather',
        ])
        assert.deepEqual(defaults, ['pisper-island', 'weather', 'weather'])
        assert.deepEqual(prefs, {
          'pisper-island': false,
          weather: true,
          timer: true,
          removed: false,
        })
        assert.deepEqual(
          resolveFloatingWidgetIds(['../escape', 'Upper', 'valid.id'], {
            '/path': true,
            unknown: 'yes',
          }),
          ['valid.id'],
        )
        let getterCalled = false
        const accessors = {}
        Object.defineProperty(accessors, 'weather', {
          enumerable: true,
          get() {
            getterCalled = true
            return false
          },
        })
        assert.deepEqual(resolveFloatingWidgetIds(['weather'], accessors), ['weather'])
        assert.equal(getterCalled, false)
      },
    )

    await t.test('user choices persist as booleans and reload without extra writes', async () => {
      store.getState().setVisible('pisper-island', false)
      store.getState().setVisible('custom.panel', true)
      assert.deepEqual(JSON.parse(values.get(STORAGE_KEY)), {
        state: { prefs: { 'pisper-island': false, 'custom.panel': true } },
        version: 1,
      })
      const before = writes
      store.getState().setVisible('custom.panel', true)
      await store.persist.rehydrate()
      assert.equal(writes, before)
      assert.deepEqual(store.getState().prefs, {
        'pisper-island': false,
        'custom.panel': true,
      })
      assert.deepEqual(resolveFloatingWidgetIds(['pisper-island'], store.getState().prefs), [
        'custom.panel',
      ])
    })

    await t.test(
      'closed defaults and other installed components can be reopened independently',
      async () => {
        await seed({ prefs: { 'pisper-island': false, weather: false } })
        const defaults = ['pisper-island']
        store.getState().setVisible('weather', true)
        assert.deepEqual(resolveFloatingWidgetIds(defaults, store.getState().prefs), ['weather'])
        store.getState().setVisible('pisper-island', true)
        assert.deepEqual(resolveFloatingWidgetIds(defaults, store.getState().prefs), [
          'pisper-island',
          'weather',
        ])
        store.getState().setVisible('pisper-island', false)
        await store.persist.rehydrate()
        assert.deepEqual(resolveFloatingWidgetIds(defaults, store.getState().prefs), ['weather'])
        assert.equal(store.getState().prefs['pisper-island'], false)
      },
    )

    await t.test('invalid inputs are rejected before changing memory or storage', () => {
      const before = store.getState().prefs
      const beforeWrites = writes
      for (const id of [
        '',
        '../escape',
        '/absolute',
        'nested/path',
        'x\\y',
        'Upper',
        'x'.repeat(65),
        null,
      ]) {
        assert.throws(() => store.getState().setVisible(id, true), { code: 'invalid_id' })
      }
      for (const value of ['true', 1, undefined, null]) {
        assert.throws(() => store.getState().setVisible('valid', value), {
          code: 'invalid_visibility',
        })
      }
      assert.equal(store.getState().prefs, before)
      assert.equal(writes, beforeWrites)
      assert.doesNotThrow(() => store.getState().setVisible('x'.repeat(64), true))
    })

    await t.test(
      'bad entries are discarded, stored size is bounded and existing entries can still change',
      async () => {
        const prefs = { 'bad/path': true, invalid: 'true', empty: null }
        for (let index = 0; index < 130; index++) prefs[`widget-${index}`] = index % 2 === 0
        const before = writes
        await seed({ prefs })
        assert.equal(Object.keys(store.getState().prefs).length, 128)
        assert.equal(Object.hasOwn(store.getState().prefs, 'bad/path'), false)
        assert.equal(Object.hasOwn(store.getState().prefs, 'invalid'), false)
        assert.equal(writes, before)
        assert.throws(() => store.getState().setVisible('new-component', true), {
          code: 'preferences_limit',
        })
        store.getState().setVisible('widget-0', false)
        assert.equal(store.getState().prefs['widget-0'], false)
        assert.equal(Object.keys(store.getState().prefs).length, 128)
      },
    )

    await t.test(
      'old or malformed envelopes fall back without overwriting their original bytes',
      async () => {
        for (const [state, version] of [
          [{ prefs: { widget: true } }, 0],
          [{ prefs: { widget: true } }, 2],
          [null, 1],
          [[], 1],
          [{ prefs: null }, 1],
          [{ prefs: [] }, 1],
          [{ prefs: { widget: true }, extra: true }, 1],
        ]) {
          const before = writes
          await seed(state, version)
          const source = JSON.stringify({ state, version })
          assert.deepEqual(store.getState().prefs, {})
          assert.equal(values.get(STORAGE_KEY), source)
          assert.equal(writes, before)
        }
        values.set(STORAGE_KEY, '{broken')
        const before = writes
        await store.persist.rehydrate()
        assert.deepEqual(store.getState().prefs, {})
        assert.equal(store.getState().storageError, true)
        assert.equal(values.get(STORAGE_KEY), '{broken')
        assert.equal(writes, before)
      },
    )

    await t.test(
      'denied storage keeps current toggles usable and reports persistence failure',
      async () => {
        await seed({ prefs: { 'pisper-island': false } })
        assert.equal(store.getState().storageError, false)
        writesDenied = true
        const before = values.get(STORAGE_KEY)
        store.getState().setVisible('weather', true)
        assert.equal(store.getState().prefs.weather, true)
        assert.equal(store.getState().storageError, true)
        assert.equal(values.get(STORAGE_KEY), before)
        writesDenied = false
        store.getState().setVisible('weather', false)
        assert.equal(store.getState().storageError, false)
        assert.equal(JSON.parse(values.get(STORAGE_KEY)).state.prefs.weather, false)
        readsDenied = true
        await store.persist.rehydrate()
        assert.deepEqual(store.getState().prefs, {})
        assert.equal(store.getState().storageError, true)
        readsDenied = false
        await store.persist.rehydrate()
        assert.equal(store.getState().storageError, false)
        assert.equal(store.getState().prefs.weather, false)
      },
    )
  } finally {
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else delete globalThis.window
  }
})
