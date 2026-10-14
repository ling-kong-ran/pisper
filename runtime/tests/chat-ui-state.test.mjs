import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { createPrimaryActionRegistry } from '../../src/app/primary-action.ts'
import {
  SESSIONS_UPDATED_EVENT,
  announceSessionsUpdated,
  subscribeSessionDeletionUpdates,
  subscribeSessionTitleUpdates,
} from '../../src/features/chat/events.ts'
import {
  applySessionTitleUpdate,
  createSessionTitleReconciler,
  mergeSessionLists,
  recentSessionCwd,
  sessionCwdForCreate,
  shouldInheritRecentSessionCwd,
  removeTiledSession,
  toggleTiledSession,
} from '../../src/features/chat/session-list.ts'

test('primary action remains callable until its page registration is disposed', () => {
  const registry = createPrimaryActionRegistry()
  let calls = 0
  const dispose = registry.register(() => {
    calls += 1
  })

  registry.invoke()
  registry.invoke()
  assert.equal(calls, 2)

  dispose()
  registry.invoke()
  assert.equal(calls, 2)
})

test('a queued primary action runs once when a lazy page registers', () => {
  const registry = createPrimaryActionRegistry()
  let calls = 0

  registry.invoke()
  registry.register(() => {
    calls += 1
  })
  assert.equal(calls, 1)
})

test('disposing an old page action does not clear the newly registered action', () => {
  const registry = createPrimaryActionRegistry()
  let calls = 0
  const disposeOld = registry.register(() => {})
  registry.register(() => {
    calls += 1
  })

  disposeOld()
  registry.invoke()
  assert.equal(calls, 1)
})

test('stale initial session lists preserve an optimistically created session', () => {
  const optimistic = { id: 'new-session', name: '新会话' }
  const stale = [{ id: 'existing-session', name: '旧会话' }]

  assert.deepEqual(mergeSessionLists([optimistic], stale), [stale[0], optimistic])
})

test('saved session title events update an open catalog twice without changing other sessions', () => {
  const priorWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  const target = new EventTarget()
  Object.defineProperty(globalThis, 'window', { configurable: true, value: target })
  const other = { id: 'other', name: 'Other chat' }
  let sessions = [{ id: 'selected', name: '新会话' }, other]
  const unsubscribe = subscribeSessionTitleUpdates(target, (update) => {
    sessions = applySessionTitleUpdate(sessions, update)
  })

  try {
    const first = 'Repeat title prefix! first'
    const second = 'Repeat title prefix! second'
    announceSessionsUpdated({ id: 'selected', name: first })
    assert.equal(sessions[0].name, first)
    announceSessionsUpdated({ id: 'selected', name: second })
    assert.equal(sessions[0].name, second)
    assert.equal(sessions[1], other)

    const unchanged = sessions
    announceSessionsUpdated()
    target.dispatchEvent(
      new CustomEvent(SESSIONS_UPDATED_EVENT, { detail: { id: 1, name: 'invalid' } }),
    )
    announceSessionsUpdated({ id: 'unknown', name: 'ignored' })
    assert.equal(sessions, unchanged)

    unsubscribe()
    announceSessionsUpdated({ id: 'selected', name: 'after unsubscribe' })
    assert.equal(sessions, unchanged)
  } finally {
    unsubscribe()
    if (priorWindow) Object.defineProperty(globalThis, 'window', priorWindow)
    else delete globalThis.window
  }
})

test('a title saved before the initial catalog is installed survives a delayed configuration response', async () => {
  const target = new EventTarget()
  const titles = createSessionTitleReconciler()
  const config = Promise.withResolvers()
  const snapshot = [{ id: 'selected', name: 'Original title' }]
  let sessions = []
  const unsubscribe = subscribeSessionTitleUpdates(target, (update) => {
    titles.record(update)
    sessions = applySessionTitleUpdate(sessions, update)
  })
  const requestRevision = titles.getRevision()
  const listResponse = Promise.resolve(snapshot)
  const initialization = Promise.all([listResponse, config.promise]).then(([incoming]) => {
    sessions = mergeSessionLists(sessions, titles.reconcile(incoming, requestRevision))
  })

  try {
    // 目录已返回、配置尚未完成时，侧栏已经可以确认重命名。
    await listResponse
    target.dispatchEvent(
      new CustomEvent(SESSIONS_UPDATED_EVENT, {
        detail: { id: 'selected', name: 'Saved while loading' },
      }),
    )
    assert.deepEqual(sessions, [])
    config.resolve({})
    await initialization
    assert.equal(sessions[0].name, 'Saved while loading')
    assert.equal(snapshot[0].name, 'Original title', 'the received snapshot is not mutated')
  } finally {
    config.resolve({})
    await initialization
    unsubscribe()
  }
})

test('two confirmed titles survive older refreshes without changing another open session or run data', () => {
  const target = new EventTarget()
  const titles = createSessionTitleReconciler()
  const active = { id: 'active', name: 'Active chat', model: 'provider/model' }
  const background = {
    id: 'background',
    name: 'Background chat',
    streaming: true,
    goal: { status: 'active', objective: 'Keep working' },
    plan: { steps: [{ title: 'Existing step', status: 'in_progress' }] },
    agents: [{ id: 'existing-agent', status: 'running' }],
  }
  const snapshot = [active, background]
  let sessions = snapshot
  const firstRequest = titles.getRevision()
  const unsubscribe = subscribeSessionTitleUpdates(target, (update) => {
    titles.record(update)
    sessions = applySessionTitleUpdate(sessions, update)
  })

  try {
    target.dispatchEvent(
      new CustomEvent(SESSIONS_UPDATED_EVENT, {
        detail: { id: 'background', name: 'First saved title' },
      }),
    )
    const secondRequest = titles.getRevision()
    const intermediateSnapshot = sessions
    target.dispatchEvent(
      new CustomEvent(SESSIONS_UPDATED_EVENT, {
        detail: { id: 'background', name: 'Second saved title' },
      }),
    )
    assert.equal(sessions[1].name, 'Second saved title')

    for (const [incoming, revision] of [
      [intermediateSnapshot, secondRequest],
      [snapshot, firstRequest],
    ]) {
      sessions = titles.reconcile(incoming, revision)
      assert.equal(sessions[1].name, 'Second saved title')
      assert.equal(sessions[0], active)
      assert.equal(sessions[1].streaming, true)
      assert.equal(sessions[1].goal, background.goal)
      assert.equal(sessions[1].plan, background.plan)
      assert.equal(sessions[1].agents, background.agents)
    }
    assert.equal(snapshot[1].name, 'Background chat')
    assert.equal(intermediateSnapshot[1].name, 'First saved title')
  } finally {
    unsubscribe()
  }
})

test('a refresh started after local renaming accepts a newer title saved by another client', () => {
  const titles = createSessionTitleReconciler()
  titles.record({ id: 'selected', name: 'Local title' })
  const requestRevision = titles.getRevision()
  const remote = { id: 'selected', name: 'Newer title from another client' }

  assert.equal(titles.reconcile([remote], requestRevision)[0], remote)
  assert.deepEqual(titles.reconcile([], requestRevision), [])
  const matching = { id: 'selected', name: 'Local title' }
  assert.equal(titles.reconcile([matching], requestRevision - 1)[0], matching)
})

test('deleted title updates are removed and disposed subscribers stop recording subsequent renames', () => {
  const target = new EventTarget()
  const titles = createSessionTitleReconciler()
  const requestRevision = titles.getRevision()
  const unsubscribeTitle = subscribeSessionTitleUpdates(target, (update) => titles.record(update))
  const unsubscribeDeletion = subscribeSessionDeletionUpdates(target, ({ deletedIds }) => {
    titles.remove(deletedIds)
  })

  try {
    for (const id of ['deleted', 'retained']) {
      target.dispatchEvent(
        new CustomEvent(SESSIONS_UPDATED_EVENT, { detail: { id, name: `Saved ${id}` } }),
      )
    }
    target.dispatchEvent(
      new CustomEvent(SESSIONS_UPDATED_EVENT, { detail: { deletedIds: ['deleted'] } }),
    )
    const snapshot = [
      { id: 'deleted', name: 'Original deleted' },
      { id: 'retained', name: 'Original retained' },
    ]
    const reconciled = titles.reconcile(snapshot, requestRevision)
    assert.equal(reconciled[0], snapshot[0], 'deletion also removes the pending title override')
    assert.equal(reconciled[1].name, 'Saved retained')

    unsubscribeTitle()
    unsubscribeDeletion()
    const revisionAtDisposal = titles.getRevision()
    target.dispatchEvent(
      new CustomEvent(SESSIONS_UPDATED_EVENT, {
        detail: { id: 'retained', name: 'After disposal' },
      }),
    )
    target.dispatchEvent(
      new CustomEvent(SESSIONS_UPDATED_EVENT, { detail: { deletedIds: ['retained'] } }),
    )
    assert.equal(titles.getRevision(), revisionAtDisposal)
    assert.equal(titles.reconcile(snapshot, requestRevision)[1].name, 'Saved retained')
  } finally {
    unsubscribeTitle()
    unsubscribeDeletion()
  }
})

test('confirmed deletion events reach open views without treating refreshes or titles as deletions', () => {
  const priorWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  const target = new EventTarget()
  Object.defineProperty(globalThis, 'window', { configurable: true, value: target })
  const received = []
  const unsubscribe = subscribeSessionDeletionUpdates(target, ({ deletedIds }) => {
    received.push(deletedIds)
  })

  try {
    announceSessionsUpdated()
    announceSessionsUpdated({ id: 'first', name: 'Renamed' })
    target.dispatchEvent(
      new CustomEvent(SESSIONS_UPDATED_EVENT, { detail: { deletedIds: ['first', 3] } }),
    )
    assert.deepEqual(received, [])

    announceSessionsUpdated({ deletedIds: ['first', 'second', 'first'] })
    assert.deepEqual(received, [['first', 'second']])

    unsubscribe()
    announceSessionsUpdated({ deletedIds: ['third'] })
    assert.deepEqual(received, [['first', 'second']])
  } finally {
    unsubscribe()
    if (priorWindow) Object.defineProperty(globalThis, 'window', priorWindow)
    else delete globalThis.window
  }
})

test('new sessions inherit the most recently listed workspace', async () => {
  const sessions = [
    { id: 'latest-without-cwd', modified: '2026-08-02T02:00:00Z', cwd: '  ' },
    { id: 'latest-workspace', modified: '2026-08-02T01:00:00Z', cwd: 'E:\\code\\latest' },
    { id: 'older-workspace', modified: '2026-08-01T01:00:00Z', cwd: 'E:\\code\\older' },
  ]
  assert.equal(recentSessionCwd(sessions), 'E:\\code\\latest')
  assert.equal(recentSessionCwd([]), '')

  // 桌面默认继承最近目录；移动本机不继承；移动远程继续继承；显式目录始终优先。
  assert.equal(shouldInheritRecentSessionCwd(false), true)
  assert.equal(shouldInheritRecentSessionCwd(true, { paired: false, mode: 'local' }), false)
  assert.equal(shouldInheritRecentSessionCwd(true, { paired: true, mode: 'remote' }), true)
  assert.equal(sessionCwdForCreate('', sessions), 'E:\\code\\latest')
  assert.equal(sessionCwdForCreate('', sessions, false), '')
  assert.equal(sessionCwdForCreate('/mobile/explicit', sessions, false), '/mobile/explicit')

  const [catalog, api] = await Promise.all([
    readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
    readFile('src/features/chat/chat-api.ts', 'utf8'),
  ])
  assert.match(catalog, /sessionCwdForCreate\(cwd, sessionsRef\.current, inheritRecentCwd\)/)
  assert.match(api, /data: \{ name, \.\.\.\(cwd \? \{ cwd \} : \{\}\) \}/)
})

test('workspace groups create chats with their exact working directory', async () => {
  // 「最近会话」区块拆分懒加载后，工作区分组与会话列表的实现在
  // SidebarRecentSessions.tsx，AppSidebar 只保留壳与懒加载入口。
  const [sidebar, sidebarShell, events, chatPage, catalog, storage, english, chinese] =
    await Promise.all([
      readFile('src/components/layout/SidebarRecentSessions.tsx', 'utf8'),
      readFile('src/components/layout/AppSidebar.tsx', 'utf8'),
      readFile('src/features/chat/events.ts', 'utf8'),
      readFile('src/features/chat/ChatPage.tsx', 'utf8'),
      readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
      readFile('src/app/storage.ts', 'utf8'),
      readFile('src/locales/en-US/navigation.json', 'utf8').then(JSON.parse),
      readFile('src/locales/zh-CN/navigation.json', 'utf8').then(JSON.parse),
    ])

  assert.match(sidebarShell, /SidebarRecentSessions/)
  assert.match(sidebar, /requestSessionCreation\(cwd\)/)
  assert.match(sidebar, /onClick=\{\(\) => createSessionInWorkspace\(group\.cwd\)\}/)
  assert.match(sidebar, /<Plus size=\{14\}/)
  assert.match(storage, /sessionCreateRequest: 'pisper-session-create-request'/)
  assert.match(events, /localStorage\.setItem\(STORAGE_KEYS\.sessionCreateRequest/)
  assert.match(events, /localStorage\.removeItem\(STORAGE_KEYS\.sessionCreateRequest\)/)
  assert.match(chatPage, /addEventListener\(SESSION_CREATE_REQUESTED_EVENT, createRequested\)/)
  assert.match(chatPage, /createSession\(undefined, request\.cwd\)/)
  assert.match(
    catalog,
    /createSessionRecord = useCallback\(\s*\(cwd = '', \{ inheritRecentCwd = true \}: CreateSessionOptions = \{\}\) =>/,
  )
  assert.match(catalog, /sessionCwdForCreate\(cwd, sessionsRef\.current, inheritRecentCwd\)/)
  assert.equal(english['appSidebar.newChatInWorkspace'], 'New chat in {workspace}')
  assert.equal(chinese['appSidebar.newChatInWorkspace'], '在 {workspace} 中新建会话')
})

test('removing a tiled session keeps the session itself available elsewhere', () => {
  assert.deepEqual(removeTiledSession(['first', 'second', 'third'], 'second'), ['first', 'third'])
})

test('a session can be added to and removed from the tiled set', () => {
  assert.deepEqual(toggleTiledSession(['first'], 'second'), ['first', 'second'])
  assert.deepEqual(toggleTiledSession(['first', 'second'], 'second'), ['first'])
})

test('the chat composer exposes the global command palette shortcut', async () => {
  const [app, events, focus, english, chinese] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/chat/events.ts', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/locales/en-US/chat.json', 'utf8').then(JSON.parse),
    readFile('src/locales/zh-CN/chat.json', 'utf8').then(JSON.parse),
  ])

  assert.match(events, /COMMAND_PALETTE_REQUESTED_EVENT/)
  assert.match(app, /addEventListener\(COMMAND_PALETTE_REQUESTED_EVENT, openCommandPalette\)/)
  assert.match(focus, /className="command-palette-trigger[^"\n]*"/)
  assert.match(focus, /onClick=\{requestCommandPalette\}/)
  assert.match(focus, /<kbd>\{COMMAND_PALETTE_SHORTCUT\}<\/kbd>/)
  assert.match(focus, /command-palette-trigger[^"\n]*\[&_kbd\]:sr-only/)
  assert.equal(
    english['focusSession.openCommandPaletteShortcut'],
    'Open command palette ({shortcut})',
  )
  assert.equal(chinese['focusSession.openCommandPaletteShortcut'], '打开命令面板（{shortcut}）')
})

test('the git changes badge uses the theme-aware contrasting text color', async () => {
  const controls = await readFile('src/features/chat/GitChangesControl.tsx', 'utf8')
  assert.match(controls, /git-changes-trigger[^"\n]*\[&_>_i\]:text-\[var\(--on-accent\)\]/)
})
