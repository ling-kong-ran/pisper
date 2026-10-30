import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  deleteSessionsSequentially,
  groupSessionsByWorkspace,
  moveWorkspaceInOrder,
  NO_WORKSPACE_KEY,
  normalizeWorkspaceOrder,
  orderWorkspaceGroups,
  recentWorkspaceGroups,
  reconcileWorkspaceOrder,
  replacementActiveSessionId,
  sessionsInWorkspace,
  sessionWorkspaceKey,
} from '../../src/features/chat/session-workspaces.ts'
import { orderVisibleSessions } from '../../src/features/chat/session-list.ts'
import {
  deleteWorkspaceProject,
  renameWorkspaceProject,
  resetWorkspaceProjectName,
} from '../../src/features/chat/project-actions.ts'
import { useWorkspaceOrderStore } from '../../src/features/chat/workspace-order-store.ts'
import { queryClient } from '../../src/lib/startup-queries.ts'

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

  // 旧侧栏（ProjectTaskList）已删除；项目删除现在只走全部代理的“管理项目”（project-actions），
  // 同样按全量目录（不受搜索与最近列表限制）计数与确认。
  const actions = await readFile('src/features/chat/project-actions.ts', 'utf8')
  assert.match(actions, /const targets = sessionsInWorkspace\(freshSessions, project\.key\)/)
  assert.match(actions, /count: targets\.length/)
  assert.match(
    actions,
    /result\.deletedIds\.length \? \{ deletedIds: result\.deletedIds \} : undefined/,
  )
  assert.match(
    actions,
    /const remainingCount = sessionsInWorkspace\(refreshed, project\.key\)\.length/,
  )
  assert.doesNotMatch(actions, /removeSessions\(/)
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

test('moving a project swaps only the listed neighbours and keeps unlisted folders in place', () => {
  const order = ['/a', '/old', '/b', '/c']
  const listed = ['/a', '/b', '/c']
  assert.deepEqual(moveWorkspaceInOrder(order, listed, '/b', -1), ['/b', '/old', '/a', '/c'])
  assert.deepEqual(moveWorkspaceInOrder(order, listed, '/b', 1), ['/a', '/old', '/c', '/b'])
  // 到边界、或不在列表里的键，不改变顺序。
  assert.deepEqual(moveWorkspaceInOrder(order, listed, '/a', -1), order)
  assert.deepEqual(moveWorkspaceInOrder(order, listed, '/c', 1), order)
  assert.deepEqual(moveWorkspaceInOrder(order, listed, '/old', 1), order)
  // 还没记过位置的项目先按列出的顺序补到末尾，再移动。
  assert.deepEqual(moveWorkspaceInOrder([], ['/x', '/y'], '/y', -1), ['/y', '/x'])
  assert.deepEqual(moveWorkspaceInOrder(['/x'], ['/x', NO_WORKSPACE_KEY], NO_WORKSPACE_KEY, -1), [
    NO_WORKSPACE_KEY,
    '/x',
  ])
})

function memoryStorage() {
  const values = new Map()
  return {
    getItem: (key) => (values.has(key) ? values.get(key) : null),
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: (key) => values.delete(key),
  }
}

// 项目操作会读写本机偏好、广播会话事件并请求 /api/sessions：Node 里用 EventTarget 充当 window。
function projectHarness(t, catalog, { failDelete = () => false } = {}) {
  const target = new EventTarget()
  target.localStorage = memoryStorage()
  target.sessionStorage = memoryStorage()
  globalThis.window = target
  const originalFetch = globalThis.fetch
  const calls = []
  globalThis.fetch = async (path, init = {}) => {
    const method = init.method || 'GET'
    calls.push({ path: String(path), method })
    const id = decodeURIComponent(String(path).split('/').pop())
    if (method === 'DELETE') {
      if (failDelete(id)) return new Response(JSON.stringify({ error: 'nope' }), { status: 500 })
      catalog.splice(
        catalog.findIndex((session) => session.id === id),
        1,
      )
      return new Response(JSON.stringify({ deleted: true, id }), { status: 200 })
    }
    return new Response(JSON.stringify({ sessions: catalog }), { status: 200 })
  }
  t.after(() => {
    globalThis.fetch = originalFetch
    delete globalThis.window
    queryClient.clear()
  })
  const events = []
  for (const name of ['pisper:sessions-updated', 'pisper:active-session-changed'])
    target.addEventListener(name, (event) => events.push([name, event.detail]))
  const notes = []
  const confirms = []
  const context = {
    t: (key, values) => (values ? `${key} ${JSON.stringify(values)}` : key),
    notify: (message, tone = 'success') => notes.push({ message, tone }),
    requestConfirm: async (options) => {
      confirms.push(options)
      return context.approve
    },
    approve: true,
  }
  return { calls, events, notes, confirms, context, storage: target.localStorage }
}

const appProject = { key: '/work/app', cwd: '/work/app', label: 'app' }

test('deleting a project rereads the full catalog and reports only confirmed deletions', async (t) => {
  const catalog = [
    { id: 'a1', cwd: '/work/app' },
    { id: 'a2', cwd: '/work/app/' },
    { id: 'a3', cwd: '/work/app' },
    { id: 'b1', cwd: '/work/other' },
  ]
  const harness = projectHarness(t, catalog, { failDelete: (id) => id === 'a2' })
  assert.equal(await deleteWorkspaceProject(appProject, harness.context), 'partial')
  // 确认框写出完整路径与完整目录里的会话数（而不是看板上看得到的那几张卡）。
  assert.match(harness.confirms[0].message, /"project":"app · \/work\/app"/)
  assert.match(harness.confirms[0].message, /"count":3/)
  // 第一个失败就停下，只把接口确认成功的计入。
  assert.deepEqual(
    harness.calls.filter((call) => call.method === 'DELETE').map((call) => call.path),
    ['/api/sessions/a1', '/api/sessions/a2'],
  )
  assert.deepEqual(harness.events[0], ['pisper:sessions-updated', { deletedIds: ['a1'] }])
  const last = harness.notes.at(-1)
  assert.equal(last.tone, 'error')
  assert.match(last.message, /projectDeletePartial/)
  assert.match(last.message, /"deleted":1/)
  assert.match(last.message, /"remaining":2/)
})

test('a confirmed project deletion clears the remembered session without opening another', async (t) => {
  const catalog = [
    { id: 'a1', cwd: '/work/app' },
    { id: 'b1', cwd: '/work/other' },
  ]
  const harness = projectHarness(t, catalog)
  harness.storage.setItem('pisper-active-session', 'a1')
  assert.equal(await deleteWorkspaceProject(appProject, harness.context), 'deleted')
  assert.equal(harness.storage.getItem('pisper-active-session'), 'b1')
  assert.deepEqual(harness.events, [
    ['pisper:sessions-updated', { deletedIds: ['a1'] }],
    ['pisper:active-session-changed', { id: 'b1', model: '' }],
  ])
  assert.match(harness.notes.at(-1).message, /projectDeleted/)
  // 不排“打开会话”的导航意图：看板上删除项目不能把人带进会话页。
  assert.equal(harness.storage.getItem('pisper-session-open-request'), null)
})

test('cancelling or deleting an empty project sends no delete requests', async (t) => {
  const catalog = [{ id: 'b1', cwd: '/work/other' }]
  const harness = projectHarness(t, catalog)
  assert.equal(await deleteWorkspaceProject(appProject, harness.context), 'empty')
  assert.equal(harness.notes.at(-1).tone, 'info')
  assert.equal(harness.confirms.length, 0)
  catalog.push({ id: 'a1', cwd: '/work/app' })
  harness.context.approve = false
  assert.equal(await deleteWorkspaceProject(appProject, harness.context), 'cancelled')
  assert.ok(!harness.calls.some((call) => call.method === 'DELETE'))
  assert.equal(catalog.length, 2)
})

test('project names are display preferences that can be renamed and reset', async (t) => {
  const harness = projectHarness(t, [])
  const answers = ['Pisper App', 'app']
  const context = { ...harness.context, requestText: async () => answers.shift() ?? null }
  try {
    assert.equal(
      await renameWorkspaceProject(appProject, 'app', context, { description: '' }),
      true,
    )
    assert.equal(useWorkspaceOrderStore.getState().names['/work/app'], 'Pisper App')
    // 改回目录名本身等于清掉自定义名。
    const renamed = { ...appProject, label: 'Pisper App' }
    assert.equal(await renameWorkspaceProject(renamed, 'app', context, { description: '' }), true)
    assert.equal(useWorkspaceOrderStore.getState().names['/work/app'], undefined)
    useWorkspaceOrderStore.getState().setWorkspaceName('/work/app', 'Custom')
    assert.equal(resetWorkspaceProjectName(appProject, context), true)
    assert.equal(useWorkspaceOrderStore.getState().names['/work/app'], undefined)
    assert.equal(resetWorkspaceProjectName(appProject, context), false)
    // “无工作区”分组没有目录可挂名字，不弹输入框。
    let prompted = false
    const noWorkspace = { key: NO_WORKSPACE_KEY, cwd: '', label: '无工作区' }
    assert.equal(
      await renameWorkspaceProject(
        noWorkspace,
        '',
        {
          ...context,
          requestText: async () => {
            prompted = true
            return 'x'
          },
        },
        { description: '' },
      ),
      false,
    )
    assert.equal(prompted, false)
  } finally {
    useWorkspaceOrderStore.setState({ names: Object.create(null) })
  }
})

test('the workspace order store moves projects among the listed keys', (t) => {
  projectHarness(t, [])
  try {
    useWorkspaceOrderStore.setState({ order: ['/a', '/b'] })
    useWorkspaceOrderStore.getState().moveWorkspace('/c', -1, ['/a', '/b', '/c'])
    assert.deepEqual(useWorkspaceOrderStore.getState().order, ['/a', '/c', '/b'])
  } finally {
    useWorkspaceOrderStore.setState({ order: [] })
  }
})
