// 登记的项目（空项目）：偏好读写按不可信数据校验、与会话目录按工作区键合并去重、
// 以及“移除项目”（只取消登记）与“连同会话一起删除”（成功后同时取消登记）的语义。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { BROWSER_PREFERENCE_KEYS } from '../../shared/browser-preferences.mjs'
import { NO_WORKSPACE_KEY } from '../../src/features/chat/session-workspaces.ts'
import {
  normalizeRegisteredProjects,
  REGISTERED_PROJECT_LIMIT,
  REGISTERED_PROJECT_PATH_MAX_LENGTH,
  registeredProjectPath,
  registerProjectPath,
  unlistedRegisteredProjects,
  unregisterProjectPath,
} from '../../src/features/chat/registered-projects.ts'
import {
  REGISTERED_PROJECTS_PREFERENCE_KEY,
  useRegisteredProjectsStore,
} from '../../src/features/chat/registered-projects-store.ts'
import {
  deleteWorkspaceProject,
  removeRegisteredProject,
} from '../../src/features/chat/project-actions.ts'
import {
  defaultLaunchCwd,
  managedProjects,
  MAX_RECENT_PROJECTS,
  recentLaunchProjects,
  withRegisteredProjects,
} from '../../src/features/board/board-projects.ts'
import { queryClient } from '../../src/lib/startup-queries.ts'

function memoryStorage() {
  const values = new Map()
  return {
    values,
    getItem: (key) => (values.has(key) ? values.get(key) : null),
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: (key) => values.delete(key),
  }
}

// 项目操作会读写本机偏好、广播会话事件并请求 /api/sessions：Node 里用 EventTarget 充当 window。
function harness(t, catalog = []) {
  const target = new EventTarget()
  target.localStorage = memoryStorage()
  target.sessionStorage = memoryStorage()
  globalThis.window = target
  const originalFetch = globalThis.fetch
  const calls = []
  globalThis.fetch = async (path, init = {}) => {
    const method = init.method || 'GET'
    calls.push({ path: String(path), method })
    if (method === 'DELETE') {
      const id = decodeURIComponent(String(path).split('/').pop())
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
    useRegisteredProjectsStore.setState({ projects: [] })
  })
  const notes = []
  const context = {
    t: (key, values) => (values ? `${key} ${JSON.stringify(values)}` : key),
    notify: (message, tone = 'success') => notes.push({ message, tone }),
    requestConfirm: async () => true,
    requestText: async () => null,
  }
  return { calls, notes, context, storage: target.localStorage }
}

test('registered project paths are trimmed, bounded and keep their spelling for dispatch', () => {
  assert.equal(registeredProjectPath('  /work/app/  '), '/work/app')
  assert.equal(registeredProjectPath('C:\\Work\\App\\'), 'C:\\Work\\App')
  // 根目录不能被去掉尾斜杠后变成“无工作区”或盘符的当前目录。
  assert.equal(registeredProjectPath('/'), '/')
  assert.equal(registeredProjectPath('C:\\'), 'C:\\')
  for (const invalid of [
    '',
    '   ',
    null,
    42,
    { cwd: '/x' },
    '/a\0b',
    `/${'x'.repeat(REGISTERED_PROJECT_PATH_MAX_LENGTH)}`,
  ])
    assert.equal(registeredProjectPath(invalid), '', String(invalid))
})

test('persisted registrations are validated as untrusted data and deduplicated by workspace key', () => {
  assert.deepEqual(normalizeRegisteredProjects(undefined), [])
  assert.deepEqual(normalizeRegisteredProjects('/work/app'), [])
  assert.deepEqual(normalizeRegisteredProjects({ 0: '/work/app' }), [])
  assert.deepEqual(
    normalizeRegisteredProjects([
      '/work/app/',
      ' /work/app ',
      'C:\\Work\\Repo',
      'c:/work/repo/',
      7,
      null,
      '',
      '/work/other',
    ]),
    ['/work/app', 'C:\\Work\\Repo', '/work/other'],
  )
  const many = Array.from({ length: REGISTERED_PROJECT_LIMIT + 10 }, (_, index) => `/p/${index}`)
  const bounded = normalizeRegisteredProjects(many)
  assert.equal(bounded.length, REGISTERED_PROJECT_LIMIT)
  assert.equal(bounded[0], '/p/0')
})

test('registering moves a project to the front and the oldest registration falls off at the limit', () => {
  assert.deepEqual(registerProjectPath(['/a', '/b'], '/b/'), ['/b', '/a'])
  assert.deepEqual(registerProjectPath(['/a'], '  '), ['/a'])
  const full = Array.from({ length: REGISTERED_PROJECT_LIMIT }, (_, index) => `/p/${index}`)
  const next = registerProjectPath(full, '/new')
  assert.equal(next.length, REGISTERED_PROJECT_LIMIT)
  assert.equal(next[0], '/new')
  assert.ok(!next.includes(`/p/${REGISTERED_PROJECT_LIMIT - 1}`))
  assert.deepEqual(unregisterProjectPath(['/a', 'C:\\Repo', '/b'], 'c:/repo/'), ['/a', '/b'])
  assert.deepEqual(unregisterProjectPath(['/a'], '/missing'), ['/a'])
})

test('registered projects merge with catalog projects without duplicates', () => {
  assert.deepEqual(unlistedRegisteredProjects(['/new', '/app/', '/new/'], ['/app']), [
    { key: '/new', cwd: '/new' },
  ])

  // 看板筛选候选：看板上的项目在前，没出现过的登记项目补在后面，计 0 张卡。
  const board = [{ key: '/app', cwd: '/app', count: 3 }]
  assert.deepEqual(withRegisteredProjects(board, ['/empty', '/app']), [
    { key: '/app', cwd: '/app', count: 3 },
    { key: '/empty', cwd: '/empty', count: 0 },
  ])

  // 启动台“最近的项目”：没有会话的登记项目排最前，其后按会话修改时间倒序；无工作区不列出。
  const sessions = [
    { id: 'a', cwd: '/app', modified: '2026-10-01T00:00:00Z' },
    { id: 'b', cwd: '/beta', modified: '2026-10-03T00:00:00Z' },
    { id: 'c', modified: '2026-10-04T00:00:00Z' },
  ]
  assert.deepEqual(recentLaunchProjects(sessions, ['/empty', '/app/'], { '/empty': '空项目' }), [
    { key: '/empty', cwd: '/empty', label: '空项目' },
    { key: '/beta', cwd: '/beta', label: 'beta' },
    { key: '/app', cwd: '/app', label: 'app' },
  ])
  // 会话项目仍只取最近的 MAX_RECENT_PROJECTS 个，登记项目不挤掉它们。
  const busy = Array.from({ length: MAX_RECENT_PROJECTS + 3 }, (_, index) => ({
    id: `s${index}`,
    cwd: `/s/${index}`,
    modified: new Date(Date.UTC(2026, 0, index + 1)).toISOString(),
  }))
  const recent = recentLaunchProjects(busy, ['/empty'], {})
  assert.equal(recent.length, MAX_RECENT_PROJECTS + 1)
  assert.equal(recent[0].cwd, '/empty')
  assert.equal(recent[1].cwd, `/s/${MAX_RECENT_PROJECTS + 2}`)
  // 隐式默认派发目录：列表里登记的空项目排在前面，但默认选中最近有会话的项目；
  // 只有登记项目、没有会话时才取第一个登记项目，什么都没有时交给 Runtime 默认目录。
  const launchList = recentLaunchProjects(sessions, ['/empty'], {})
  assert.equal(launchList[0].cwd, '/empty')
  assert.equal(defaultLaunchCwd(sessions, launchList), '/beta')
  assert.equal(defaultLaunchCwd([], recentLaunchProjects([], ['/empty'], {})), '/empty')
  assert.equal(defaultLaunchCwd([], []), '')

  // 管理项目：会话目录与登记合并，没有会话的显示 0 个会话；顺序与显示名照常生效。
  const managed = managedProjects(
    [{ id: 'a1', cwd: '/app' }, { id: 'a2', cwd: '/app/' }, { id: 'n1' }],
    ['/empty', '/app'],
    { '/empty': 'Fresh' },
    ['/empty', '/app'],
    '无项目',
  )
  assert.deepEqual(
    managed.map(({ key, label, count, custom }) => ({ key, label, count, custom })),
    [
      { key: '/empty', label: 'Fresh', count: 0, custom: true },
      { key: '/app', label: 'app', count: 2, custom: false },
      { key: NO_WORKSPACE_KEY, label: '无项目', count: 1, custom: false },
    ],
  )
})

test('the registered project store persists a validated list under its registered key', async (t) => {
  const { storage } = harness(t)
  assert.equal(REGISTERED_PROJECTS_PREFERENCE_KEY, 'pisper-registered-projects')
  assert.ok(BROWSER_PREFERENCE_KEYS.includes(REGISTERED_PROJECTS_PREFERENCE_KEY))
  const store = useRegisteredProjectsStore
  assert.equal(store.getState().registerProject(' /work/app/ '), '/work/app')
  assert.equal(store.getState().registerProject('   '), '')
  store.getState().registerProject('/work/next')
  assert.deepEqual(store.getState().projects, ['/work/next', '/work/app'])
  const saved = JSON.parse(storage.getItem(REGISTERED_PROJECTS_PREFERENCE_KEY))
  assert.deepEqual(saved.state, { projects: ['/work/next', '/work/app'] })

  // 磁盘或其他端口写回来的内容按不可信数据重新校验。
  storage.setItem(
    REGISTERED_PROJECTS_PREFERENCE_KEY,
    JSON.stringify({
      state: { projects: ['/ok', 42, '/ok/', '', { cwd: '/x' }, '/a\0b'] },
      version: 1,
    }),
  )
  await store.persist.rehydrate()
  assert.deepEqual(store.getState().projects, ['/ok'])
  storage.setItem(REGISTERED_PROJECTS_PREFERENCE_KEY, JSON.stringify({ state: { projects: 'x' } }))
  await store.persist.rehydrate()
  assert.deepEqual(store.getState().projects, [])

  assert.equal(store.getState().unregisterProject('/missing'), false)
})

test('removing an empty registered project only unregisters it and sends no requests', async (t) => {
  const { calls, notes, context } = harness(t, [{ id: 'b1', cwd: '/work/other' }])
  useRegisteredProjectsStore.setState({ projects: ['/work/empty', '/work/other'] })
  const project = { key: '/work/empty', cwd: '/work/empty', label: 'empty' }
  assert.equal(removeRegisteredProject(project, context), true)
  assert.deepEqual(useRegisteredProjectsStore.getState().projects, ['/work/other'])
  assert.equal(calls.length, 0)
  assert.match(notes.at(-1).message, /appSidebar\.projectRemoved/)
  assert.match(notes.at(-1).message, /empty · \/work\/empty/)
  // 没登记过的目录：什么也不做，也不提示。
  assert.equal(removeRegisteredProject(project, context), false)
  assert.equal(notes.length, 1)
  assert.equal(
    removeRegisteredProject({ key: NO_WORKSPACE_KEY, cwd: '', label: '' }, context),
    false,
  )
})

test('deleting a project with chats also unregisters it, but only after every chat is gone', async (t) => {
  const catalog = [
    { id: 'a1', cwd: '/work/app' },
    { id: 'b1', cwd: '/work/other' },
  ]
  const { calls, context } = harness(t, catalog)
  useRegisteredProjectsStore.setState({ projects: ['/work/app/', '/work/other'] })
  const app = { key: '/work/app', cwd: '/work/app', label: 'app' }
  assert.equal(await deleteWorkspaceProject(app, context), 'deleted')
  assert.deepEqual(
    calls.filter((call) => call.method === 'DELETE').map((call) => call.path),
    ['/api/sessions/a1'],
  )
  assert.deepEqual(useRegisteredProjectsStore.getState().projects, ['/work/other'])

  // 取消删除时登记保持不变。
  const other = { key: '/work/other', cwd: '/work/other', label: 'other' }
  assert.equal(
    await deleteWorkspaceProject(other, { ...context, requestConfirm: async () => false }),
    'cancelled',
  )
  assert.deepEqual(useRegisteredProjectsStore.getState().projects, ['/work/other'])
})

test('board project lists read registrations through the shared store and merge helpers', async () => {
  const read = (path) => readFile(path, 'utf8')
  const [launch, page, dialog, commands] = await Promise.all([
    read('src/features/board/useLaunchOptions.ts'),
    // 全部代理与手机看板的项目列表都经 useLabeledProjects 合并登记项目（A3 起）。
    read('src/features/board/useLabeledProjects.ts'),
    read('src/features/board/ManageProjectsDialog.tsx'),
    read('src/features/board/useProjectCommands.ts'),
  ])
  assert.match(launch, /recentLaunchProjects\(list \?\? \[\], registered, names\)/)
  assert.match(launch, /defaultLaunchCwd\(list \?\? \[\], projects\)/)
  assert.match(page, /withRegisteredProjects\(\[\.\.\.projects\], registered\)/)
  assert.match(page, /useRegisteredProjectsStore\(\(state\) => state\.projects\)/)
  assert.match(dialog, /managedProjects\(/)
  // 没有会话的行只取消登记；有会话的才走“连同会话一起删除”。
  assert.match(dialog, /if \(project\.count === 0\) commands\.unregister\(project\)/)
  assert.match(dialog, /board:project\.removeNamed/)
  assert.match(dialog, /board:project\.deleteWithChatsNamed/)
  // 新建项目选完目录（系统对话框或手动填写）都先登记，再选为派发目标。
  assert.match(commands, /registerProject\(picked\)/)
})
