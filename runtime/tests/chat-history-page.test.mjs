import assert from 'node:assert/strict'
import test from 'node:test'
import {
  canSplitHistorySessions,
  HISTORY_BATCH_SIZE,
  selectHistorySessions,
} from '../../src/features/chat/history-list.ts'

test('history split actions follow Dock capability on compact and native mobile layouts', () => {
  assert.equal(canSplitHistorySessions(false, false), true)
  assert.equal(canSplitHistorySessions(true, false), false)
  assert.equal(canSplitHistorySessions(false, true), false)
  assert.equal(canSplitHistorySessions(true, true), false)
})

test('history searches the full catalog before rendering a bounded batch', () => {
  const sessions = Array.from({ length: 1_000 }, (_, index) => ({
    id: `session-${index}`,
    name: index === 999 ? 'Late match' : `Session ${index}`,
    cwd: index === 998 ? '/workspace/target' : '/workspace/other',
  }))
  const first = selectHistorySessions(sessions, '', HISTORY_BATCH_SIZE)
  assert.equal(first.total, 1_000)
  assert.equal(first.items.length, 50)
  assert.equal(first.items.at(-1)?.id, 'session-49')

  const second = selectHistorySessions(sessions, '', HISTORY_BATCH_SIZE * 2)
  assert.equal(second.items.length, 100)
  assert.equal(second.items.at(50)?.id, 'session-50')

  const titleMatch = selectHistorySessions(sessions, ' LATE MATCH ', HISTORY_BATCH_SIZE)
  assert.equal(titleMatch.total, 1)
  assert.equal(titleMatch.items[0]?.id, 'session-999')
  const workspaceMatch = selectHistorySessions(sessions, 'target', HISTORY_BATCH_SIZE)
  assert.equal(workspaceMatch.total, 1)
  assert.equal(workspaceMatch.items[0]?.id, 'session-998')
})

test('history separates archived chats but still searches both views and pins within the result', () => {
  const sessions = [
    { id: 'recent', name: 'Recent', modified: '2026-01-03T00:00:00Z' },
    { id: 'archived', name: 'Archived result', modified: '2026-01-04T00:00:00Z', archived: true },
    { id: 'pinned', name: 'Pinned', modified: '2026-01-01T00:00:00Z', pinned: true },
  ]
  assert.deepEqual(
    selectHistorySessions(sessions, '', 50, 'active').items.map((session) => session.id),
    ['pinned', 'recent'],
  )
  assert.deepEqual(
    selectHistorySessions(sessions, '', 50, 'archived').items.map((session) => session.id),
    ['archived'],
  )
  assert.deepEqual(
    selectHistorySessions(sessions, 'result', 50, 'active').items.map((session) => session.id),
    ['archived'],
  )
})
