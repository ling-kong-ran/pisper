import assert from 'node:assert/strict'
import test from 'node:test'
import { resolveTaskOpen, classifyDockSnapshot } from '../../src/features/chat/dock-layout.ts'

const base = {
  sessionId: 'c',
  visibleIds: ['a', 'b'],
  focusedId: 'a',
  disposition: 'open',
  width: 900,
  height: 700,
}
test('task selection replaces only the focused pane and visible tasks are focused without moving', () => {
  assert.deepEqual(resolveTaskOpen(base), { kind: 'replace', replacedId: 'a' })
  assert.deepEqual(resolveTaskOpen({ ...base, sessionId: 'b' }), { kind: 'focus' })
  assert.deepEqual(resolveTaskOpen({ ...base, sessionId: 'b', disposition: 'right' }), {
    kind: 'focus',
  })
})
test('explicit splits preserve other tasks and reject insufficient space without replacing', () => {
  for (const disposition of ['left', 'right', 'above', 'below'])
    assert.deepEqual(resolveTaskOpen({ ...base, disposition }), { kind: 'split' })
  assert.deepEqual(resolveTaskOpen({ ...base, disposition: 'right', width: 500 }), {
    kind: 'unavailable',
  })
  assert.deepEqual(resolveTaskOpen({ ...base, disposition: 'below', height: 300 }), {
    kind: 'unavailable',
  })
  assert.deepEqual(resolveTaskOpen({ ...base, visibleIds: [], focusedId: '' }), { kind: 'add' })
})
test('unknown or corrupt saved layouts prohibit overwriting while empty preferences allow a fresh layout', () => {
  assert.equal(classifyDockSnapshot(null).writable, true)
  assert.equal(classifyDockSnapshot({ version: 999, layout: {} }).writable, false)
  assert.equal(classifyDockSnapshot('{broken').writable, false)
})
