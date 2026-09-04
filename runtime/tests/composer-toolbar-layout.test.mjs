import assert from 'node:assert/strict'
import test from 'node:test'
import {
  COMPOSER_TOOL_IDS,
  allocateComposerToolbar,
  moveComposerTool,
  normalizeComposerToolbarLayout,
  setComposerToolLocation,
} from '../../src/features/chat/composer-toolbar-layout.ts'

test('composer toolbar layout repairs stale, duplicate, and unknown tool ids', () => {
  const layout = normalizeComposerToolbarLayout({
    inline: ['model', 'model', 'unknown', 'attachment'],
    overflow: ['attachment', 'commands', 12],
  })

  assert.deepEqual(layout.inline.slice(0, 2), ['model', 'attachment'])
  assert.deepEqual(layout.overflow, ['commands'])
  assert.equal(new Set([...layout.inline, ...layout.overflow]).size, COMPOSER_TOOL_IDS.length)
  assert.deepEqual(new Set([...layout.inline, ...layout.overflow]), new Set(COMPOSER_TOOL_IDS))
})

test('composer tools move between locations and reorder without affecting other tools', () => {
  const initial = normalizeComposerToolbarLayout(null)
  const overflowed = setComposerToolLocation(initial, 'attachment', 'overflow')
  assert.equal(overflowed.inline.includes('attachment'), false)
  assert.deepEqual(overflowed.overflow, ['attachment'])

  const moved = moveComposerTool(overflowed, 'model', -1)
  assert.ok(moved.inline.indexOf('model') < overflowed.inline.indexOf('model'))
  assert.deepEqual(moved.overflow, ['attachment'])

  const restored = setComposerToolLocation(moved, 'attachment', 'inline')
  assert.equal(restored.overflow.includes('attachment'), false)
  assert.equal(restored.inline.at(-1), 'attachment')
})

test('composer allocation preserves user overflow and temporarily overflows the inline tail', () => {
  const layout = setComposerToolLocation(
    normalizeComposerToolbarLayout(null),
    'session-actions',
    'overflow',
  )
  const available = ['attachment', 'resource', 'model', 'commands', 'session-actions']
  const allocation = allocateComposerToolbar(layout, available, 3)

  assert.deepEqual(allocation.inline, ['attachment', 'resource', 'model'])
  assert.deepEqual(allocation.automaticallyOverflowed, ['commands'])
  assert.deepEqual(allocation.overflow, ['commands', 'session-actions'])
})

test('unavailable capabilities do not mutate or leak into the rendered allocation', () => {
  const layout = normalizeComposerToolbarLayout({
    inline: ['visual', 'git-changes', 'attachment'],
    overflow: ['commands'],
  })
  const allocation = allocateComposerToolbar(layout, ['attachment', 'commands'], Infinity)

  assert.deepEqual(allocation.inline, ['attachment'])
  assert.deepEqual(allocation.overflow, ['commands'])
  assert.equal(layout.inline.includes('visual'), true)
  assert.equal(layout.inline.includes('git-changes'), true)
})
