import assert from 'node:assert/strict'
import test from 'node:test'
import {
  COMPOSER_TOOL_IDS,
  DEFAULT_COMPOSER_TOOLBAR_LAYOUT,
  allocateComposerToolbar,
  moveComposerTool,
  normalizeComposerToolbarLayout,
  setAllComposerToolsLocation,
  setComposerToolLocation,
} from '../../src/features/chat/composer-toolbar-layout.ts'

test('composer toolbar layout repairs stale, duplicate, and unknown tool ids', () => {
  const layout = normalizeComposerToolbarLayout({
    inline: ['model', 'model', 'unknown', 'attachment'],
    overflow: ['attachment', 'commands', 12],
  })

  assert.deepEqual(layout.inline, ['model', 'attachment', 'permission', 'run-mode'])
  assert.equal(layout.overflow[0], 'commands')
  assert.equal(new Set([...layout.inline, ...layout.overflow]).size, COMPOSER_TOOL_IDS.length)
  assert.deepEqual(new Set([...layout.inline, ...layout.overflow]), new Set(COMPOSER_TOOL_IDS))
})

test('new composer preferences keep readable permission, Plan and combined model/reasoning inline', () => {
  const initial = normalizeComposerToolbarLayout(null)
  assert.deepEqual(initial, DEFAULT_COMPOSER_TOOLBAR_LAYOUT)
  assert.deepEqual(initial.inline, ['permission', 'run-mode', 'model'])
  assert.deepEqual(new Set([...initial.inline, ...initial.overflow]), new Set(COMPOSER_TOOL_IDS))
  assert.notEqual(initial.inline, DEFAULT_COMPOSER_TOOLBAR_LAYOUT.inline)
})

test('composer tools move between locations and reorder without affecting other tools', () => {
  const initial = normalizeComposerToolbarLayout({ inline: COMPOSER_TOOL_IDS, overflow: [] })
  const overflowed = setComposerToolLocation(initial, 'resource', 'overflow')
  assert.deepEqual(overflowed.overflow, ['resource'])
  assert.deepEqual(
    overflowed.inline,
    initial.inline.filter((id) => id !== 'resource'),
  )

  const restored = setComposerToolLocation(overflowed, 'resource', 'inline')
  assert.deepEqual(restored.overflow, [])
  assert.equal(restored.inline.at(-1), 'resource')

  const moved = moveComposerTool(restored, 'resource', -1)
  assert.deepEqual(moved.inline.slice(-2), ['resource', 'session-actions'])
  assert.deepEqual(moved.inline.slice(0, -2), restored.inline.slice(0, -2))
})

test('bulk placement preserves every tool and its order through storage and restoration', () => {
  const initial = setComposerToolLocation(normalizeComposerToolbarLayout(null), 'model', 'overflow')
  const before = structuredClone(initial)
  const expected = [...initial.inline, ...initial.overflow]
  const stored = setAllComposerToolsLocation(initial, 'overflow')
  assert.deepEqual(stored, { inline: [], overflow: expected })
  const restored = setAllComposerToolsLocation(stored, 'inline')
  assert.deepEqual(restored, { inline: expected, overflow: [] })
  assert.deepEqual(setAllComposerToolsLocation(restored, 'inline'), restored)
  assert.deepEqual(initial, before)
})

test('composer allocation preserves user overflow and temporarily overflows the inline tail', () => {
  const layout = normalizeComposerToolbarLayout({
    inline: ['attachment', 'resource', 'commands'],
    overflow: ['session-actions'],
  })
  const available = ['attachment', 'resource', 'commands', 'session-actions']
  const allocation = allocateComposerToolbar(layout, available, 2)

  assert.deepEqual(allocation.inline, ['attachment', 'resource'])
  assert.deepEqual(allocation.automaticallyOverflowed, ['commands'])
  assert.equal(allocation.overflow.includes('session-actions'), true)
})

test('legacy twelve-tool defaults remove retired controls and keep remaining tools inline', () => {
  const legacy = normalizeComposerToolbarLayout({
    inline: [
      'attachment',
      'resource',
      'visual',
      'model',
      'permission',
      'run-mode',
      'thinking',
      'commands',
      'git-changes',
      'file-changes',
      'compact-context',
      'session-actions',
    ],
    overflow: [],
  })
  assert.deepEqual(legacy, { inline: [...COMPOSER_TOOL_IDS], overflow: [] })
  assert.equal(COMPOSER_TOOL_IDS.includes('model'), true)
  assert.equal(COMPOSER_TOOL_IDS.includes('permission'), true)
  assert.equal(COMPOSER_TOOL_IDS.includes('run-mode'), true)
  assert.equal(COMPOSER_TOOL_IDS.includes('git-changes'), false)
  assert.equal(COMPOSER_TOOL_IDS.includes('file-changes'), false)
})

test('retired change controls are filtered from both locations without resetting custom order', () => {
  const previous = {
    inline: ['commands', 'git-changes', 'attachment', 'file-changes', 'thinking'],
    overflow: [
      'visual',
      'file-changes',
      'permission',
      'model',
      'git-changes',
      'resource',
      'run-mode',
      'session-actions',
      'compact-context',
    ],
  }
  const before = structuredClone(previous)
  const migrated = normalizeComposerToolbarLayout(previous)
  assert.deepEqual(migrated, {
    inline: ['commands', 'attachment'],
    overflow: [
      'visual',
      'permission',
      'model',
      'resource',
      'run-mode',
      'session-actions',
      'compact-context',
    ],
  })
  assert.deepEqual(normalizeComposerToolbarLayout(migrated), migrated)
  assert.deepEqual(previous, before)
  assert.deepEqual(allocateComposerToolbar(migrated, COMPOSER_TOOL_IDS, Infinity), {
    ...migrated,
    automaticallyOverflowed: [],
  })
})

test('custom placements survive with restored controls remaining independently configurable', () => {
  const custom = normalizeComposerToolbarLayout({
    inline: ['commands', 'model', 'attachment'],
    overflow: ['visual', 'run-mode', 'permission'],
  })
  assert.deepEqual(custom.inline, ['commands', 'model', 'attachment'])
  assert.deepEqual(custom.overflow.slice(0, 3), ['visual', 'run-mode', 'permission'])
  assert.deepEqual(normalizeComposerToolbarLayout(custom), custom)
})

test('preferences restore missing runtime controls inline and retain every saved placement', () => {
  const restoredIds = ['model', 'permission', 'run-mode']
  const previousIds = COMPOSER_TOOL_IDS.filter((id) => !restoredIds.includes(id))
  const previous = { inline: ['commands', 'attachment'], overflow: previousIds.slice().reverse() }
  previous.overflow = previous.overflow.filter((id) => !previous.inline.includes(id))
  const migrated = normalizeComposerToolbarLayout(previous)

  assert.deepEqual(migrated.inline, [...previous.inline, ...restoredIds])
  assert.deepEqual(migrated.overflow, previous.overflow)
  assert.deepEqual(normalizeComposerToolbarLayout(migrated), migrated)

  const previousDefault = normalizeComposerToolbarLayout({ inline: [], overflow: previousIds })
  assert.deepEqual(previousDefault.inline, restoredIds)
  assert.deepEqual(previousDefault.overflow, previousIds)
})

test('temporarily overflowed controls return when space grows without changing saved locations', () => {
  const layout = setComposerToolLocation(
    normalizeComposerToolbarLayout({ inline: COMPOSER_TOOL_IDS, overflow: [] }),
    'permission',
    'overflow',
  )
  const before = structuredClone(layout)
  const narrow = allocateComposerToolbar(layout, COMPOSER_TOOL_IDS, 2)
  assert.deepEqual(narrow.inline, ['attachment', 'resource'])
  assert.equal(narrow.automaticallyOverflowed.includes('model'), true)
  assert.equal(narrow.automaticallyOverflowed.includes('permission'), false)

  const wide = allocateComposerToolbar(layout, COMPOSER_TOOL_IDS, Infinity)
  assert.deepEqual(wide.inline, layout.inline)
  assert.deepEqual(wide.overflow, ['permission'])
  assert.deepEqual(wide.automaticallyOverflowed, [])
  assert.deepEqual(layout, before)
})

test('unavailable capabilities do not mutate or leak into the rendered allocation', () => {
  const layout = normalizeComposerToolbarLayout({
    inline: ['visual', 'model', 'attachment'],
    overflow: ['commands'],
  })
  const allocation = allocateComposerToolbar(layout, ['attachment', 'commands'], Infinity)

  assert.deepEqual(allocation.inline, ['attachment'])
  assert.deepEqual(allocation.overflow, ['commands'])
  assert.equal(layout.inline.includes('visual'), true)
  assert.equal(layout.inline.includes('model'), true)
})

test('readable controls use weighted widths without losing tools or rewriting preferences', () => {
  const layout = normalizeComposerToolbarLayout(null)
  const before = structuredClone(layout)
  const widths = { permission: 2.4, 'run-mode': 1.4, model: 2.7 }
  assert.deepEqual(allocateComposerToolbar(layout, COMPOSER_TOOL_IDS, 6.5, widths).inline, [
    'permission',
    'run-mode',
    'model',
  ])
  const small = allocateComposerToolbar(layout, COMPOSER_TOOL_IDS, 5.1, widths)
  assert.deepEqual(small.inline, ['permission', 'run-mode'])
  assert.deepEqual(small.automaticallyOverflowed, ['model'])
  assert.deepEqual(new Set([...small.inline, ...small.overflow]), new Set(COMPOSER_TOOL_IDS))
  assert.deepEqual(allocateComposerToolbar(layout, COMPOSER_TOOL_IDS, 0, widths).inline, [])
  assert.deepEqual(layout, before)
})

test('legacy separate effort shortcut retires without losing combined model access or mutating preferences', () => {
  const previous = {
    inline: ['thinking', 'permission', 'model'],
    overflow: ['commands', 'run-mode'],
  }
  const before = structuredClone(previous)
  const migrated = normalizeComposerToolbarLayout(previous)
  assert.deepEqual(migrated.inline, ['permission', 'model'])
  assert.equal([...migrated.inline, ...migrated.overflow].includes('thinking'), false)
  assert.deepEqual(normalizeComposerToolbarLayout(migrated), migrated)
  assert.deepEqual(previous, before)
  assert.deepEqual(new Set([...migrated.inline, ...migrated.overflow]), new Set(COMPOSER_TOOL_IDS))
})
