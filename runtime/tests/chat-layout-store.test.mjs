import assert from 'node:assert/strict'
import test from 'node:test'
import {
  CHAT_LAYOUT_SAVED_LIMIT,
  DEFAULT_CHAT_LAYOUT,
} from '../../src/features/chat/layout/chat-layout.ts'

const STORAGE_KEY = 'pisper-chat-layout'
const CONTEXT_KEY = 'pisper-session-context-layout'
const layout = (name, overrides = {}) => ({
  ...structuredClone(DEFAULT_CHAT_LAYOUT),
  name,
  ...overrides,
})

test('chat layout preferences preserve user layouts, validate reloads and survive denied storage', async (t) => {
  const values = new Map([[CONTEXT_KEY, JSON.stringify({ state: { width: 512 }, version: 1 })]])
  let readFails = false
  let writeFails = false
  let writes = 0
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
    const { useChatLayoutStore: store } =
      await import('../../src/features/chat/layout/chat-layout-store.ts')
    const { useSessionContextStore: context } =
      await import('../../src/features/chat/session-context-store.ts')
    const seed = async (state, version = 1) => {
      values.set(STORAGE_KEY, JSON.stringify({ state, version }))
      await store.persist.rehydrate()
    }

    await t.test('initial defaults do not overwrite existing context width or storage', () => {
      assert.deepEqual(store.getState().active, DEFAULT_CHAT_LAYOUT)
      assert.deepEqual(store.getState().saved, [])
      assert.equal(store.getState().revision, 0)
      assert.equal(store.getState().storageError, false)
      assert.equal(context.getState().width, 512)
      assert.equal(writes, 0)
    })

    await t.test('apply validates and copies the template, applying context width once', () => {
      const template = layout('My layout')
      template.desktop.contextWidth = 600
      store.getState().apply(template)
      assert.equal(store.getState().revision, 1)
      assert.equal(context.getState().width, 600)
      template.desktop.contextWidth = 300
      assert.equal(store.getState().active.desktop.contextWidth, 600)
      context.getState().setWidth(480)
      assert.equal(context.getState().width, 480)
      assert.equal(store.getState().active.desktop.contextWidth, 600)
      assert.deepEqual(Object.keys(JSON.parse(values.get(STORAGE_KEY)).state).sort(), [
        'active',
        'saved',
      ])
      assert.throws(() => store.getState().apply({ ...template, accent: 'unsafe' }), {
        code: 'invalid_value',
      })
      assert.equal(store.getState().revision, 1)
      assert.equal(context.getState().width, 480)
    })

    await t.test('saving only changes saved templates and repeated names preserve ids', () => {
      const active = store.getState().active
      const revision = store.getState().revision
      const id = store.getState().save(layout('Focus desk'))
      const replacement = layout('  FOCUS DESK  ')
      replacement.desktop.composerPosition = 'top'
      assert.equal(store.getState().save(replacement), id)
      assert.equal(store.getState().saved.length, 1)
      assert.equal(store.getState().saved[0].template.desktop.composerPosition, 'top')
      assert.equal(store.getState().active, active)
      assert.equal(store.getState().revision, revision)
      assert.equal(context.getState().width, 480)
    })

    await t.test(
      'reload restores active and saved data without resetting dragged width',
      async () => {
        const stored = values.get(STORAGE_KEY)
        const expected = JSON.parse(stored).state
        store.getState().apply(layout('Temporary'))
        context.getState().setWidth(500)
        values.set(STORAGE_KEY, stored)
        const beforeReload = writes
        await store.persist.rehydrate()
        assert.deepEqual(store.getState().active, expected.active)
        assert.deepEqual(store.getState().saved, expected.saved)
        assert.equal(context.getState().width, 500)
        assert.equal(writes, beforeReload)
      },
    )

    await t.test(
      'twenty presets may be saved and same-name replacement still works at capacity',
      async () => {
        await seed({ active: DEFAULT_CHAT_LAYOUT, saved: [] })
        for (let index = 0; index < CHAT_LAYOUT_SAVED_LIMIT; index++)
          store.getState().save(layout(`Layout ${index}`))
        assert.equal(
          new Set(store.getState().saved.map((entry) => entry.id)).size,
          CHAT_LAYOUT_SAVED_LIMIT,
        )
        assert.throws(() => store.getState().save(layout('Too many')), { code: 'saved_limit' })
        assert.doesNotThrow(() => store.getState().save(layout('Layout 0')))
        const [first] = store.getState().saved
        store.getState().remove(first.id)
        assert.equal(store.getState().saved.length, CHAT_LAYOUT_SAVED_LIMIT - 1)
        const before = writes
        store.getState().remove('missing')
        assert.equal(writes, before)
        assert.doesNotThrow(() => store.getState().save(layout('Room again')))
      },
    )

    await t.test('reset restores active defaults and width while retaining saved presets', () => {
      const saved = store.getState().saved
      const revision = store.getState().revision
      store.getState().reset()
      assert.deepEqual(store.getState().active, DEFAULT_CHAT_LAYOUT)
      assert.equal(context.getState().width, 360)
      assert.equal(store.getState().saved, saved)
      assert.equal(store.getState().revision, revision + 1)
    })

    await t.test(
      'invalid stored entries cannot inject state or break remaining valid layouts',
      async () => {
        const valid = { id: 'layout-good', template: layout('Good') }
        await seed({
          active: { ...DEFAULT_CHAT_LAYOUT, version: 900 },
          saved: [
            null,
            { id: 'bad id', template: layout('Invalid ID') },
            { id: 'layout-corrupt', template: { ...layout('Corrupt'), script: 'untrusted' } },
            valid,
            { ...valid, template: layout('Duplicate ID') },
            { id: 'layout-duplicate', template: layout('GOOD') },
            { id: 'layout-actions', template: layout('Extra fields'), apply: 'untrusted' },
          ],
        })
        assert.deepEqual(store.getState().active, DEFAULT_CHAT_LAYOUT)
        assert.deepEqual(store.getState().saved, [valid])
        assert.equal(typeof store.getState().apply, 'function')
        for (const state of [
          null,
          [],
          'bad',
          { active: layout('Injected'), saved: [], revision: 100 },
        ]) {
          await seed(state)
          assert.deepEqual(store.getState().active, DEFAULT_CHAT_LAYOUT)
          assert.deepEqual(store.getState().saved, [])
        }
      },
    )

    await t.test('unsupported storage versions are ignored without rewriting them', async () => {
      for (const version of [0, 2, '1', null]) {
        await seed({ active: layout('Future'), saved: [] }, version)
        const source = values.get(STORAGE_KEY)
        assert.deepEqual(store.getState().active, DEFAULT_CHAT_LAYOUT)
        assert.equal(values.get(STORAGE_KEY), source)
      }
    })

    await t.test(
      'existing v1 active and saved templates migrate on read and write v2 on apply',
      async () => {
        const legacy = layout('Legacy')
        legacy.version = 1
        delete legacy.desktop.canvas
        delete legacy.mobile.canvas
        legacy.desktop.composerPosition = 'top'
        await seed({ active: legacy, saved: [{ id: 'legacy', template: legacy }] })
        assert.equal(store.getState().active.version, 2)
        assert.equal(store.getState().saved[0].template.version, 2)
        assert.deepEqual(
          store.getState().active.desktop.canvas.children.map((node) => node.kind),
          ['header', 'composer', 'messages'],
        )
        assert.equal(JSON.parse(values.get(STORAGE_KEY)).state.active.version, 1)
        store.getState().apply(store.getState().active)
        const persisted = JSON.parse(values.get(STORAGE_KEY))
        assert.equal(persisted.version, 1)
        assert.equal(persisted.state.active.version, 2)
        assert.equal(persisted.state.saved[0].template.version, 2)
      },
    )

    await t.test(
      'malformed JSON and denied reads fall back safely and report storage failure',
      async () => {
        values.set(STORAGE_KEY, '{bad json')
        await assert.doesNotReject(store.persist.rehydrate())
        assert.deepEqual(store.getState().active, DEFAULT_CHAT_LAYOUT)
        assert.equal(store.getState().storageError, true)
        store.getState().apply(layout('Recovered'))
        assert.equal(store.getState().storageError, false)
        readFails = true
        await assert.doesNotReject(store.persist.rehydrate())
        assert.deepEqual(store.getState().active, DEFAULT_CHAT_LAYOUT)
        assert.equal(store.getState().storageError, true)
        readFails = false
      },
    )

    await t.test(
      'write failures preserve memory state without recursive writes or lost actions',
      () => {
        const saved = values.get(STORAGE_KEY)
        writeFails = true
        assert.doesNotThrow(() => store.getState().apply(layout('In memory')))
        assert.equal(store.getState().active.name, 'In memory')
        assert.equal(store.getState().storageError, true)
        assert.equal(values.get(STORAGE_KEY), saved)
        assert.doesNotThrow(() => store.getState().save(layout('Memory preset')))
        assert.equal(store.getState().saved.length, 1)
        assert.doesNotThrow(() => store.persist.clearStorage())
        writeFails = false
        store.getState().apply(layout('Saved again'))
        assert.equal(store.getState().storageError, false)
        assert.equal(JSON.parse(values.get(STORAGE_KEY)).state.active.name, 'Saved again')
      },
    )

    await t.test('denied localStorage getters keep controls usable', () => {
      Object.defineProperty(globalThis, 'window', {
        configurable: true,
        value: {
          get localStorage() {
            throw new Error('Denied')
          },
        },
      })
      assert.doesNotThrow(() => store.getState().reset())
      assert.equal(store.getState().storageError, true)
      assert.deepEqual(store.getState().active, DEFAULT_CHAT_LAYOUT)
    })
  } finally {
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})
