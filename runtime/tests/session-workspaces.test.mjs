import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  deleteSessionsSequentially,
  groupSessionsByWorkspace,
  normalizeWorkspaceOrder,
  orderWorkspaceGroups,
  recentWorkspaceGroups,
  reconcileWorkspaceOrder,
  replacementActiveSessionId,
  sessionsInWorkspace,
  sessionWorkspaceKey,
} from '../../src/features/chat/session-workspaces.ts'
import { orderVisibleSessions } from '../../src/features/chat/session-list.ts'

test('new projects remain in the sidebar when pinned chats fill the recent limit', () => {
  const sessions = [
    ...Array.from({ length: 30 }, (_, index) => ({ id: `pinned-${index}`, cwd: '/alpha' })),
    { id: 'new-project', cwd: '/beta' },
    { id: 'other-project', cwd: '/gamma' },
  ]
  const groups = recentWorkspaceGroups(sessions, 24, 'new-project')
  assert.deepEqual(
    groups.map((group) => group.key),
    ['/alpha', '/beta', '/gamma'],
  )
  assert.equal(groups[0].sessions.length, 24)
  assert.deepEqual(
    groups[1].sessions.map((session) => session.id),
    ['new-project'],
  )
  assert.deepEqual(recentWorkspaceGroups(sessions, 24, 'pinned-0'), groups)
  assert.equal(sessions.length, 32)
})

test('the active chat stays accessible without bypassing the sidebar search candidates', () => {
  const sessions = Array.from({ length: 30 }, (_, index) => ({
    id: `chat-${index}`,
    cwd: '/alpha',
  }))
  const groups = recentWorkspaceGroups(sessions, 24, 'chat-29')
  assert.equal(groups[0].sessions.length, 25)
  assert.equal(groups[0].sessions.at(-1)?.id, 'chat-29')
  assert.deepEqual(recentWorkspaceGroups(sessions.slice(0, 1), 24, 'chat-29')[0].sessions, [
    sessions[0],
  ])
  assert.deepEqual(recentWorkspaceGroups([], 24, 'chat-29'), [])
})

test('new, updated, or pinned sessions keep their workspace in its saved position', () => {
  const sessions = [
    { id: 'a', cwd: '/alpha', modified: '2026-09-24T10:00:00Z' },
    { id: 'b', cwd: '/beta', modified: '2026-09-24T09:00:00Z' },
  ]
  const order = reconcileWorkspaceOrder(
    [],
    groupSessionsByWorkspace(sessions).map((g) => g.key),
  )
  for (const changed of [
    [...sessions, { id: 'b-new', cwd: '/beta', modified: '2026-09-24T11:00:00Z' }],
    sessions.map((s) => (s.id === 'b' ? { ...s, modified: '2026-09-24T11:00:00Z' } : s)),
    sessions.map((s) => (s.id === 'b' ? { ...s, pinned: true } : s)),
  ]) {
    const groups = orderWorkspaceGroups(
      groupSessionsByWorkspace(orderVisibleSessions(changed)),
      order,
    )
    assert.deepEqual(
      groups.map((g) => g.key),
      ['/alpha', '/beta'],
    )
    assert.deepEqual(
      groups[1].sessions.map((s) => s.id),
      changed.length === 3 ? ['b-new', 'b'] : ['b'],
    )
  }
  const withNewWorkspace = groupSessionsByWorkspace(
    orderVisibleSessions([
      ...sessions,
      { id: 'c', cwd: '/gamma', modified: '2026-09-24T12:00:00Z' },
    ]),
  )
  assert.deepEqual(
    orderWorkspaceGroups(withNewWorkspace, order).map((g) => g.key),
    ['/alpha', '/beta', '/gamma'],
  )
  assert.deepEqual(
    reconcileWorkspaceOrder(
      order,
      withNewWorkspace.map((g) => g.key),
    ),
    ['/alpha', '/beta', '/gamma'],
  )
  assert.deepEqual(order, ['/alpha', '/beta'])
})

test('hidden workspaces retain positions through filtering, archival, empty results and restoration', () => {
  const order = ['/alpha', '/beta', '/gamma']
  for (const keys of [[], ['/beta'], ['/gamma', '/alpha']]) {
    assert.deepEqual(reconcileWorkspaceOrder(order, keys), order)
  }
  const groups = groupSessionsByWorkspace([
    { id: 'c', cwd: '/gamma' },
    { id: 'a', cwd: '/alpha' },
    { id: 'b', cwd: '/beta' },
  ])
  assert.deepEqual(
    orderWorkspaceGroups(groups.slice(0, 2), order).map((g) => g.key),
    ['/alpha', '/gamma'],
  )
  assert.deepEqual(
    orderWorkspaceGroups(groups, order).map((g) => g.key),
    order,
  )
  assert.deepEqual(
    groups.map((g) => g.key),
    ['/gamma', '/alpha', '/beta'],
  )
})

test('workspace order validates saved keys without duplicating Windows aliases or dropping the no-workspace group', () => {
  const noWorkspace = sessionWorkspaceKey({ id: 'none' })
  assert.deepEqual(normalizeWorkspaceOrder({ order: ['/alpha'] }), [])
  assert.deepEqual(
    normalizeWorkspaceOrder([
      'C:\\Work\\Project\\',
      'c:/work/project',
      '',
      12,
      null,
      {},
      noWorkspace,
      'invalid\0path',
      '/alpha',
      '/alpha/',
    ]),
    ['c:/work/project', noWorkspace, '/alpha'],
  )
})

test('workspace ordering survives persistence and records new workspaces only once', async () => {
  const values = new Map()
  let writes = 0
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => {
      values.set(key, value)
      writes += 1
    },
    removeItem: (key) => values.delete(key),
  }
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { localStorage: storage },
  })
  try {
    const { useWorkspaceOrderStore: store } =
      await import('../../src/features/chat/workspace-order-store.ts')
    store.getState().rememberWorkspaces(['/alpha', '/beta'])
    const saved = values.get('pisper-workspace-order')
    store.getState().rememberWorkspaces(['/beta', '/alpha'])
    store.getState().rememberWorkspaces([])
    assert.equal(writes, 1)
    store.setState({ order: [] })
    values.set('pisper-workspace-order', saved)
    await store.persist.rehydrate()
    store.getState().rememberWorkspaces(['/gamma', '/beta', '/alpha'])
    assert.deepEqual(store.getState().order, ['/alpha', '/beta', '/gamma'])
    assert.deepEqual(JSON.parse(values.get('pisper-workspace-order')).state.order, [
      '/alpha',
      '/beta',
      '/gamma',
    ])
    values.set(
      'pisper-workspace-order',
      JSON.stringify({ state: { order: [null, 42, '/beta', '/beta'] }, version: 1 }),
    )
    await store.persist.rehydrate()
    assert.deepEqual(store.getState().order, ['/beta'])
  } finally {
    if (previous) Object.defineProperty(globalThis, 'window', previous)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})

test('project membership and confirmation count use the full directory despite search and recent limits', async () => {
  const sessions = Array.from({ length: 48 }, (_, index) => ({
    id: `chat-${index}`,
    name: index === 30 ? 'needle' : `Chat ${index}`,
    cwd: index % 4 === 0 ? '/other' : index % 2 === 0 ? 'C:/Work/Project/' : 'c:\\work\\project',
  }))
  const key = sessionWorkspaceKey(sessions[1])
  const visible = sessions.filter((session) => session.name.includes('needle')).slice(0, 24)

  assert.equal(
    groupSessionsByWorkspace(sessions).find((group) => group.key === key)?.sessions.length,
    36,
  )
  assert.equal(groupSessionsByWorkspace(visible).length, 1)
  assert.equal(sessionsInWorkspace(sessions, key).length, 36)
  assert.equal(sessionsInWorkspace(sessions.slice(0, 24), key).length, 18)
  assert.notEqual(
    sessionWorkspaceKey({ id: 'no-cwd' }),
    sessionWorkspaceKey({ id: 'relative', cwd: '__no_workspace__' }),
  )

  const sidebar = await readFile('src/components/layout/SidebarRecentSessions.tsx', 'utf8')
  assert.match(sidebar, /const targets = sessionsInWorkspace\(freshSessions, group\.key\)/)
  assert.match(sidebar, /count: targets\.length/)
  assert.match(
    sidebar,
    /result\.deletedIds\.length \? \{ deletedIds: result\.deletedIds \} : undefined/,
  )
  assert.match(
    sidebar,
    /const remainingCount = sessionsInWorkspace\(result\.sessions, group\.key\)\.length/,
  )
  assert.doesNotMatch(sidebar, /removeSessions\(group\.sessions\)/)
})

test('partial project deletion reports only confirmed successes and replaces a deleted active session', async () => {
  const sessions = Array.from({ length: 34 }, (_, index) => ({
    id: `project-${index}`,
    cwd: '/project',
  }))
  sessions.push({ id: 'other-workspace', cwd: '/other' })
  const targets = sessionsInWorkspace(sessions, sessionWorkspaceKey(sessions[0]))
  const attempts = []
  const result = await deleteSessionsSequentially(
    targets.map((session) => session.id),
    async (id) => {
      attempts.push(id)
      if (id === 'project-17') throw new Error('connection lost')
    },
  )
  const deleted = new Set(result.deletedIds)
  const remaining = sessions.filter((session) => !deleted.has(session.id))

  assert.equal(result.deletedIds.length, 17)
  assert.equal(result.failedId, 'project-17')
  assert.equal(result.error?.message, 'connection lost')
  assert.equal(attempts.length, 18)
  assert.equal(sessionsInWorkspace(remaining, sessionWorkspaceKey(sessions[0])).length, 17)
  assert.equal(replacementActiveSessionId('project-1', remaining, deleted, true), 'project-17')
  assert.equal(replacementActiveSessionId('project-17', remaining, deleted, true), null)
  assert.equal(replacementActiveSessionId('other-workspace', remaining, deleted, true), null)
  assert.equal(replacementActiveSessionId('project-1', remaining, deleted, false), 'project-17')
})

test('complete deletion has no remainder and an empty directory clears the active selection', async () => {
  const ids = ['first', 'second']
  const result = await deleteSessionsSequentially(ids, async () => undefined)
  assert.deepEqual(result, { deletedIds: ids, failedId: null, error: null })
  assert.equal(replacementActiveSessionId('first', [], new Set(result.deletedIds), true), '')
})

test('a rejected deletion without an Error value cannot be mistaken for success', async () => {
  const result = await deleteSessionsSequentially(['first', 'second'], async () => {
    throw undefined
  })
  assert.equal(result.failedId, 'first')
  assert.deepEqual(result.deletedIds, [])
})
