import assert from 'node:assert/strict'
import test from 'node:test'
import {
  canSplitHistorySessions,
  HISTORY_BATCH_SIZE,
  HISTORY_PROJECT_PREVIEW,
  selectHistoryProjects,
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

test('history groups by project when none is picked and filters to one project', () => {
  const sessions = [
    { id: 'a-old', cwd: '/work/a', modified: '2026-01-01T00:00:00Z' },
    { id: 'b-new', cwd: '/work/b', modified: '2026-01-05T00:00:00Z' },
    { id: 'a-pin', cwd: '/work/a', modified: '2025-12-01T00:00:00Z', pinned: true },
    { id: 'loose', cwd: '/tmp/x', projectless: true, modified: '2026-01-03T00:00:00Z' },
    { id: 'bot', cwd: '/tmp/wt', projectCwd: '/work/b', modified: '2026-01-02T00:00:00Z' },
    ...Array.from({ length: 6 }, (_, index) => ({
      id: `a-${index}`,
      cwd: '/work/a/',
      modified: `2026-01-0${index + 1}T01:00:00Z`,
    })),
  ]
  const grouped = selectHistoryProjects(sessions, '', 'active')
  assert.equal(grouped.total, sessions.length)
  // 最近有更新的项目在前；置顶只在项目内部靠前；无项目会话归成一组。
  assert.deepEqual(
    grouped.groups.map((group) => [group.key, group.count]),
    [
      ['/work/a', 8],
      ['/work/b', 2],
      ['\0', 1],
    ],
  )
  const a = grouped.groups[0]
  assert.equal(a.items.length, HISTORY_PROJECT_PREVIEW)
  assert.deepEqual(
    a.items.map((session) => session.id),
    ['a-pin', 'a-5', 'a-4', 'a-3', 'a-2'],
  )
  assert.deepEqual(
    grouped.groups[1].items.map((session) => session.id),
    ['b-new', 'bot'],
  )

  const onlyB = selectHistorySessions(sessions, '', 50, 'active', { project: '/work/b' })
  assert.deepEqual(
    onlyB.items.map((session) => session.id),
    ['b-new', 'bot'],
  )
  assert.equal(
    selectHistorySessions(sessions, '', 50, 'active', { project: '\0' }).items[0]?.id,
    'loose',
  )
})

test('history search also matches the last message summary', () => {
  const sessions = [
    { id: 'one', name: 'Fix', lastMessage: { role: 'assistant', text: 'Pushed the rebase' } },
    { id: 'two', name: 'Other' },
  ]
  assert.deepEqual(
    selectHistorySessions(sessions, 'rebase', 50).items.map((session) => session.id),
    ['one'],
  )
  assert.deepEqual(
    selectHistoryProjects(sessions, 'rebase').groups.map((group) => group.count),
    [1],
  )
})
