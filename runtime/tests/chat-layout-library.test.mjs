import assert from 'node:assert/strict'
import test from 'node:test'
import {
  CHAT_LAYOUT_PRESETS,
  CHAT_LAYOUT_SAVED_LIMIT,
  DEFAULT_CHAT_LAYOUT,
  equalChatLayoutContent,
  parseChatLayout,
  parseChatLayoutJson,
  serializeChatLayout,
} from '../../src/features/chat/layout/chat-layout.ts'
import { canvasHasKind } from '../../src/features/chat/layout/chat-canvas.ts'
import { parseCanvasCss } from '../../src/features/chat/layout/chat-canvas-style.ts'
import { resolveSessionContextPresentation } from '../../src/features/chat/session-context-layout.ts'

const layout = (name) => ({ ...structuredClone(DEFAULT_CHAT_LAYOUT), name })
const STORAGE_KEY = 'pisper-chat-layout'

test('content matching ignores the display name and property order while preserving layout differences', () => {
  const original = layout('Original')
  const renamed = layout('Renamed')
  renamed.desktop = Object.fromEntries(Object.entries(renamed.desktop).reverse())
  renamed.mobile.canvas = Object.fromEntries(Object.entries(renamed.mobile.canvas).reverse())
  assert.equal(equalChatLayoutContent(original, renamed), true)
  renamed.mobile.fontSize = 18
  assert.equal(equalChatLayoutContent(original, renamed), false)
  const restyled = layout('Original')
  restyled.desktop.canvas.css += 'padding: 10px;'
  assert.equal(equalChatLayoutContent(original, restyled), false)
  const moved = layout('Original')
  moved.desktop.canvas.children.reverse()
  assert.equal(equalChatLayoutContent(original, moved), false)
})

test('Studio uses an independent theme-aware canvas and a responsive external context', () => {
  const studio = CHAT_LAYOUT_PRESETS.find((preset) => preset.id === 'studio').template
  assert.deepEqual(parseChatLayoutJson(serializeChatLayout(studio)), studio)
  assert.equal(equalChatLayoutContent(studio, DEFAULT_CHAT_LAYOUT), false)
  assert.equal(studio.desktop.contentWidth, 960)
  assert.equal(studio.desktop.contextSide, 'right')
  assert.equal(studio.desktop.contextVisibility, 'auto')
  assert.notDeepEqual(studio.desktop.canvas, studio.mobile.canvas)
  const colors = []
  const visit = (node) => {
    const style = parseCanvasCss(node.css)
    for (const key of ['color', 'background', 'backgroundColor', 'borderColor'])
      if (style[key]) colors.push(style[key])
    node.children?.forEach(visit)
  }
  for (const viewport of [studio.desktop, studio.mobile]) {
    assert.equal(viewport.canvas.kind, 'column')
    assert.ok(canvasHasKind(viewport.canvas, 'header'))
    assert.ok(canvasHasKind(viewport.canvas, 'messages'))
    assert.ok(canvasHasKind(viewport.canvas, 'composer'))
    assert.equal(canvasHasKind(viewport.canvas, 'context'), false)
    visit(viewport.canvas)
  }
  assert.ok(colors.length > 0)
  assert.ok(colors.every((color) => color.includes('var(--')))
  for (const [width, expected] of [
    [600, 'closed'],
    [700, 'closed'],
    [900, 'aside'],
    [1100, 'aside'],
  ]) {
    assert.equal(
      resolveSessionContextPresentation({
        availableWidth: width,
        mobileLayout: false,
        hasSession: true,
        preference: studio.desktop.contextVisibility,
      }),
      expected,
    )
  }
  assert.equal(
    resolveSessionContextPresentation({
      availableWidth: 390,
      mobileLayout: true,
      hasSession: true,
      preference: 'open',
    }),
    'sheet',
  )
})

test('the saved layout library supports non-destructive import, rename and persistence', async (t) => {
  const values = new Map()
  let writesBlocked = false
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: {
      localStorage: {
        getItem: (key) => values.get(key) ?? null,
        setItem: (key, value) => {
          if (writesBlocked) throw new Error('Storage unavailable')
          values.set(key, value)
        },
        removeItem: (key) => values.delete(key),
      },
    },
  })
  try {
    const { useChatLayoutStore: store } =
      await import('../../src/features/chat/layout/chat-layout-store.ts')
    const { useSessionContextStore: context } =
      await import('../../src/features/chat/session-context-store.ts')
    const clear = async () => {
      values.set(
        STORAGE_KEY,
        JSON.stringify({ state: { active: DEFAULT_CHAT_LAYOUT, saved: [] }, version: 1 }),
      )
      await store.persist.rehydrate()
    }

    await t.test(
      'every import appends an independent template without replacing or applying previous ones',
      () => {
        const active = store.getState().active
        const revision = store.getState().revision
        const ids = [store.getState().importTemplate(layout('Desk'))]
        ids.push(store.getState().importTemplate(layout('desk')))
        ids.push(store.getState().importTemplate(layout('Desk')))
        assert.deepEqual(
          store.getState().saved.map((entry) => entry.template.name),
          ['Desk', 'desk (2)', 'Desk (3)'],
        )
        assert.equal(new Set(ids).size, 3)
        assert.equal(store.getState().active, active)
        assert.equal(store.getState().revision, revision)
        const first = store.getState().saved[0]
        const changed = { ...first.template, accent: 'teal' }
        assert.equal(store.getState().save(changed), first.id)
        assert.equal(store.getState().saved.length, 3)
        assert.equal(store.getState().saved[0].template.accent, 'teal')
      },
    )

    await t.test(
      'long Unicode names leave room for unique suffixes without truncating surrogate pairs',
      async () => {
        await clear()
        const name = '🌟'.repeat(80)
        store.getState().importTemplate(layout(name))
        store.getState().importTemplate(layout(name))
        const duplicate = store.getState().saved[1].template
        assert.equal([...duplicate.name].length, 80)
        assert.equal(duplicate.name, `${'🌟'.repeat(76)} (2)`)
        assert.doesNotThrow(() => parseChatLayout(duplicate))
      },
    )

    await t.test(
      'renaming the current saved template preserves ids, canvas, revision and dragged context width',
      async () => {
        await clear()
        const id = store.getState().importTemplate(layout('Working'))
        const template = store.getState().saved[0].template
        store.getState().apply(template)
        context.getState().setWidth(575)
        const revision = store.getState().revision
        const active = store.getState().active
        store.getState().rename(id, '  My studio  ')
        assert.equal(store.getState().saved[0].id, id)
        assert.equal(store.getState().saved[0].template.name, 'My studio')
        assert.equal(store.getState().active.name, 'My studio')
        assert.equal(store.getState().active.desktop, active.desktop)
        assert.equal(store.getState().revision, revision)
        assert.equal(context.getState().width, 575)
        assert.equal(equalChatLayoutContent(store.getState().active, active), true)
      },
    )

    await t.test(
      'renaming another copy does not rename the active template with identical content',
      () => {
        const id = store.getState().importTemplate(layout('Copy'))
        const active = store.getState().active
        store.getState().rename(id, 'Other copy')
        assert.equal(store.getState().active, active)
        assert.equal(
          store.getState().saved.find((entry) => entry.id === id).template.name,
          'Other copy',
        )
      },
    )

    await t.test('invalid and colliding names fail atomically and missing ids are reported', () => {
      const before = store.getState()
      const id = before.saved[0].id
      assert.throws(() => before.rename(id, 'other COPY'), { code: 'duplicate_name' })
      assert.throws(() => before.rename(id, '  '), { code: 'invalid_value' })
      assert.throws(() => before.rename(id, 'a'.repeat(81)), { code: 'invalid_value' })
      assert.throws(() => before.rename('missing', 'New name'), { code: 'not_found' })
      assert.equal(store.getState(), before)
      before.rename(id, 'MY STUDIO')
      assert.equal(store.getState().saved[0].template.name, 'MY STUDIO')
      assert.equal(store.getState().active.name, 'MY STUDIO')
    })

    await t.test(
      'reload preserves renamed active and saved templates and all library actions',
      async () => {
        const saved = structuredClone({
          active: store.getState().active,
          saved: store.getState().saved,
        })
        await store.persist.rehydrate()
        assert.deepEqual(store.getState().active, saved.active)
        assert.deepEqual(store.getState().saved, saved.saved)
        assert.equal(typeof store.getState().importTemplate, 'function')
        assert.equal(typeof store.getState().rename, 'function')
        assert.equal(context.getState().width, 575)
      },
    )

    await t.test(
      'renaming an edited library version does not change the independently applied snapshot',
      async () => {
        await clear()
        const id = store.getState().importTemplate(layout('Snapshot'))
        store.getState().apply(store.getState().saved[0].template)
        context.getState().setWidth(560)
        const active = store.getState().active
        const revision = store.getState().revision
        store.getState().save({ ...active, accent: 'teal' })
        store.getState().rename(id, 'Edited library version')
        assert.equal(store.getState().saved[0].template.name, 'Edited library version')
        assert.equal(store.getState().active, active)
        assert.equal(store.getState().active.name, 'Snapshot')
        assert.equal(store.getState().active.accent, 'inherit')
        assert.equal(store.getState().revision, revision)
        assert.equal(context.getState().width, 560)
      },
    )

    await t.test(
      'import capacity never causes replacement, while existing names can still be saved',
      async () => {
        await clear()
        for (let index = 0; index < CHAT_LAYOUT_SAVED_LIMIT; index++)
          store.getState().importTemplate(layout('Collection'))
        const saved = store.getState().saved
        assert.throws(() => store.getState().importTemplate(layout('Collection')), {
          code: 'saved_limit',
        })
        assert.equal(store.getState().saved, saved)
        assert.equal(store.getState().save(saved[0].template), saved[0].id)
        assert.equal(store.getState().saved.length, CHAT_LAYOUT_SAVED_LIMIT)
      },
    )

    await t.test('storage denial keeps imported and renamed entries usable in memory', async () => {
      await clear()
      writesBlocked = true
      const id = store.getState().importTemplate(layout('Local'))
      store.getState().rename(id, 'Local only')
      assert.equal(store.getState().saved[0].template.name, 'Local only')
      assert.equal(store.getState().storageError, true)
      writesBlocked = false
    })
  } finally {
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})
