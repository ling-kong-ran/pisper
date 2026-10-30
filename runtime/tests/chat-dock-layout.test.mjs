import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  closeMobileSessionTab,
  createDockLayoutEnvelope,
  createSessionOpenRequest,
  dockPositionForDisposition,
  initialDockSessionIds,
  panelIdForSession,
  parseDockLayoutEnvelope,
  parseSessionOpenRequest,
  sessionIdFromPanel,
  sessionIdsFromDockLayout,
} from '../../src/features/chat/dock-layout.ts'

test('session panel ids round-trip through dock panel metadata', () => {
  assert.equal(panelIdForSession('alpha'), 'session:alpha')
  assert.equal(sessionIdFromPanel('session:alpha'), 'alpha')
  assert.equal(sessionIdFromPanel({ id: 'ignored', params: { sessionId: 'beta' } }), 'beta')
  assert.equal(sessionIdFromPanel({ id: 'other:alpha' }), '')
})

test('dock layout envelopes reject incompatible or malformed state', () => {
  const layout = { grid: { root: 'group-1' }, panels: {} }
  const envelope = createDockLayoutEnvelope(layout, 'session:alpha')
  assert.deepEqual(parseDockLayoutEnvelope(JSON.stringify(envelope)), envelope)
  assert.equal(parseDockLayoutEnvelope('{bad json'), null)
  assert.equal(parseDockLayoutEnvelope({ ...envelope, version: 2 }), null)
  assert.equal(parseDockLayoutEnvelope({ ...envelope, engine: 'other' }), null)
  assert.equal(parseDockLayoutEnvelope({ ...envelope, layout: [] }), null)
})

test('mobile tabs flatten only session panels from the persisted desktop layout', () => {
  assert.deepEqual(
    sessionIdsFromDockLayout({
      panels: {
        'session:alpha': { id: 'session:alpha', contentComponent: 'session' },
        'web-preview': { id: 'web-preview', contentComponent: 'webPreview' },
        'session:beta': { id: 'ignored', params: { sessionId: 'beta' } },
      },
    }),
    ['alpha', 'beta'],
  )
  assert.deepEqual(sessionIdsFromDockLayout({ panels: [] }), [])
})

test('closing a mobile tab selects its right neighbor before falling back left', () => {
  assert.deepEqual(closeMobileSessionTab(['alpha', 'beta', 'gamma'], 'beta', 'beta'), {
    tabIds: ['alpha', 'gamma'],
    activeId: 'gamma',
  })
  assert.deepEqual(closeMobileSessionTab(['alpha', 'beta'], 'beta', 'beta'), {
    tabIds: ['alpha'],
    activeId: 'alpha',
  })
  assert.deepEqual(closeMobileSessionTab(['alpha'], 'alpha', 'alpha'), {
    tabIds: [],
    activeId: '',
  })
  assert.deepEqual(closeMobileSessionTab(['alpha', 'beta'], 'alpha', 'beta'), {
    tabIds: ['beta'],
    activeId: 'beta',
  })
})

test('session open requests accept horizontal and vertical dispositions', () => {
  assert.equal(dockPositionForDisposition('left'), 'left')
  assert.equal(dockPositionForDisposition('right'), 'right')
  assert.equal(dockPositionForDisposition('above'), 'top')
  assert.equal(dockPositionForDisposition('below'), 'bottom')
  assert.deepEqual(createSessionOpenRequest('alpha', 'left'), {
    sessionId: 'alpha',
    disposition: 'left',
  })
  assert.deepEqual(createSessionOpenRequest('alpha', 'above'), {
    sessionId: 'alpha',
    disposition: 'above',
  })
  assert.deepEqual(createSessionOpenRequest('alpha', 'open', 'entry-1'), {
    sessionId: 'alpha',
    disposition: 'open',
    targetEntryId: 'entry-1',
  })
  assert.deepEqual(parseSessionOpenRequest('{"sessionId":"beta","disposition":"right"}'), {
    sessionId: 'beta',
    disposition: 'right',
  })
  assert.deepEqual(
    parseSessionOpenRequest('{"sessionId":"beta","disposition":"below","targetEntryId":"entry-2"}'),
    {
      sessionId: 'beta',
      disposition: 'below',
      targetEntryId: 'entry-2',
    },
  )
  assert.equal(createSessionOpenRequest('alpha', 'top'), null)
  assert.equal(parseSessionOpenRequest('{"sessionId":"alpha","disposition":"bottom"}'), null)
})

// 第 3 期退役 Dockview 的挂载（规格 10 第 3 期）：会话页（/board/:sessionId）是唯一挂载聊天页的路由，
// 只挂 URL 指定的一个会话；桌面布局里会话面板换成“页头 + 对话 + 检查器标签”的会话工作区，
// 分屏与多标签不再出现在任何入口（ChatDockView / use-chat-dock 的分屏代码留到第 6 期清理时删除）。
test('the session page mounts one session with the inspector, offers no dock splits and keeps mobile session management', async () => {
  const [
    dock,
    focus,
    dockHook,
    history,
    page,
    catalog,
    app,
    header,
    actions,
    shell,
    routes,
    workspace,
  ] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/use-chat-dock.ts', 'utf8'),
    readFile('src/features/chat/ChatHistoryPage.tsx', 'utf8'),
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/layout/PageHeader.tsx', 'utf8'),
    readFile('src/features/chat/SessionActionsMenu.tsx', 'utf8'),
    readFile('src/app/SessionShell.tsx', 'utf8'),
    readFile('src/app/route-elements.tsx', 'utf8'),
    readFile('src/features/chat/SessionWorkspace.tsx', 'utf8'),
  ])
  // 聊天页只由会话路由经 SessionShell 挂载，并且总带着路由会话 id：聊天页永远处于路由模式。
  assert.equal(routes.match(/import\('@\/features\/chat\/ChatPage'\)/g)?.length, 1)
  assert.match(
    routes.slice(routes.indexOf('export async function sessionRoute')),
    /^export async function sessionRoute\(\) \{[\s\S]*?import\('@\/features\/chat\/ChatPage'\)/,
  )
  assert.match(shell, /<ChatPage[\s\S]*?routeSessionId=\{sessionId\}/)
  assert.match(page, /const routeMode = routeSessionId !== undefined/)
  // 路由模式下分屏永远不可用，会话面板固定为单会话、不可分屏、不可关闭。
  assert.match(page, /splitAvailable: !mobileLayout && !routeMode/)
  assert.match(page, /singleSessionLayout: mobileLayout \|\| routeMode/)
  assert.match(page, /openRoutedSession: routeMode \? onRouteSessionChange : undefined/)
  assert.doesNotMatch(page, /ChatDockView/)
  assert.match(
    page,
    /\) : routeTarget \? \(\s*<SingleSessionPanel\s+sessionId=\{routeTarget\}\s+onCreateSession=\{createSession\}\s*\/>/,
  )
  assert.match(page, /<SingleSessionPanel onCreateSession=\{createSession\} \/>/)
  const singlePanel = dock.slice(
    dock.indexOf('export function SingleSessionPanel'),
    dock.indexOf('type MobileSessionPanelProps'),
  )
  assert.match(singlePanel, /key=\{activeId\}/)
  assert.match(singlePanel, /canSplitPanel=\{false\}/)
  assert.match(singlePanel, /canClosePanel=\{false\}/)
  // 分屏菜单项只在 canSplit 时出现（会话页从不传 true）。
  assert.match(actions, /canSplit &&[\s\n]*splitActions\.map/)
  assert.doesNotMatch(focus, /onSplitTop|onSplitBottom|<SessionActionsMenu/)
  // 会话页布局：面板渲染会话工作区（页头 + 对话 + 检查器），其余入口才是旧的任务工作区。
  assert.match(dock, /\{pageChrome \? \(\s*\/\/[^\n]*\n\s*<SessionWorkspace/)
  assert.match(dock, /<TaskWorkspace/)
  assert.match(workspace, /const SessionInspector = lazy\(/)
  assert.match(workspace, /<SessionInspector\s+panelId=\{panelId\}\s+sessionId=\{sessionId\}/)
  // 改名与换工作区：旧顶栏与会话页头都直接可点。
  assert.match(focus, /onClick=\{onRename\}/)
  assert.match(focus, /onClick=\{onWorkspace\}/)
  assert.match(focus, /onRename=\{onRename\}\s+onWorkspace=\{onWorkspace\}/)
  // 历史页打开会话只发选中请求，不提供分屏。
  assert.match(history, /<DropdownMenu>/)
  assert.match(history, /requestSessionSelection\(id\)/)
  assert.doesNotMatch(history, /splitTo(?:Left|Right|Top|Bottom)/)
  assert.match(page, /<MobileSessionPanel/)
  // 单会话布局下“打开会话”不创建 Dock 面板：路由模式交回路由切换，其余只切换活动会话。
  assert.match(
    dockHook,
    /if \(singleSessionLayoutRef\.current\) \{\s*if \(sessionId && openRoutedSessionRef\.current\) \{\s*openRoutedSessionRef\.current\(sessionId\)\s*return true/,
  )
  assert.match(
    dockHook,
    /if \(singleSessionLayoutRef\.current\) \{[\s\S]*?setActiveId\(sessionId\)[\s\S]*?return true/,
  )
  assert.match(dock, /role="tablist"/)
  assert.match(dock, /role="tab"/)
  assert.match(dock, /onOpenHistory/)
  assert.match(dock, /onCreateSession/)
  assert.match(dock, /closeMobileSessionTab/)
  assert.match(dock, /chat:chatPage\.closeChat/)
  assert.match(page, /waitForMobileRuntimeReady/)
  assert.match(page, /mobile_state/)
  assert.match(page, /shouldInheritRecentSessionCwd\(mobileApp, mobileState\)/)
  assert.match(page, /inheritRecentCwd/)
  assert.match(catalog, /inheritRecentCwd = true/)
  assert.match(catalog, /sessionCwdForCreate\(cwd, sessionsRef\.current, inheritRecentCwd\)/)
  assert.match(page, /preserveExistingOnEmpty: true/)
  assert.match(app, /mobileApp=\{mobileApp\}/)
  assert.match(header, /page === 'chat'[\s\S]*mobileApp[\s\S]*newChat/)
  assert.match(header, /page === 'chat' && mobileApp && 'hidden'/)
  assert.match(dockHook, /mobileOpenedSessionIds/)
  assert.match(dock, /activeSession\?\.id \|\| visibleTabs\[0\]\?\.id/)
  assert.doesNotMatch(focus, /WorkspaceTrustNotice/)
})

test('移动 WebView 在 API 握手前也稳定识别为移动客户端', async () => {
  const [mobileShell, clientStore] = await Promise.all([
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('src/stores/client-store.ts', 'utf8'),
  ])
  assert.match(mobileShell, /MOBILE_CLIENT_INITIALIZATION_SCRIPT/)
  assert.match(mobileShell, /__PISPER_MOBILE_APP__/)
  assert.match(mobileShell, /\.initialization_script\(MOBILE_CLIENT_INITIALIZATION_SCRIPT\)/)
  assert.match(clientStore, /window\.__PISPER_MOBILE_APP__ === true/)
  assert.match(clientStore, /client: isNativeMobileApp\(\) \? 'mobile-app' : 'web'/)
  assert.match(clientStore, /nativeMobileApp \? 'mobile-app' : 'web'/)
})

test('移动端前台恢复会先校准本机 Runtime，再保留瞬时空目录', async () => {
  const [page, catalog, shell] = await Promise.all([
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
  ])
  assert.match(page, /await waitForMobileRuntimeReady\(\)/)
  assert.match(page, /refreshSessions\(undefined, \{ preserveExistingOnEmpty: true \}\)/)
  assert.match(catalog, /preserveExistingOnEmpty = false/)
  assert.match(catalog, /!data\.sessions\.length && sessionsRef\.current\.length/)
  assert.match(shell, /async fn mobile_resume_local_runtime[\s\S]*resume_remote_network\(\)\.await/)
  assert.match(shell, /ensure_local_runtime_ready\(state\.on_device\.clone\(\)/)
  assert.match(
    shell,
    /RunEvent::Resumed[\s\S]*resume_remote_network\(\)\.await[\s\S]*ensure_local_runtime_ready/,
  )
})

// 第 3 期起 Dockview 不再挂载，这里钉住的是存量保护：只有真正初始化过的 Dock 才会写回布局，
// 会话页（从不初始化 Dock）不会覆盖用户在 Runtime 里保存的旧分屏布局，回滚到旧版本时仍可恢复。
test('dock layout persistence flushes before suspension through the Runtime queue', async () => {
  const dockHook = await readFile('src/features/chat/use-chat-dock.ts', 'utf8')
  assert.match(dockHook, /const persistDockLayout = useCallback/)
  assert.match(dockHook, /apiJson<unknown>\('\/api\/settings\/chat-dock-layout'\)/)
  assert.match(dockHook, /pendingLayoutWriteRef\.current = serialized/)
  assert.match(dockHook, /method: 'PUT'/)
  assert.doesNotMatch(dockHook, /STORAGE_KEYS\.(?:chatDockLayout|tiledSessions)/)
  assert.match(
    dockHook,
    /const envelope = createDockLayoutEnvelope\(api\.toJSON\(\), api\.activePanel\?\.id \|\| ''\)/,
  )
  assert.match(dockHook, /document\.addEventListener\('visibilitychange', flushWhenHidden\)/)
  assert.match(dockHook, /window\.addEventListener\('pagehide', flushBeforePageHide\)/)
  assert.match(
    dockHook,
    /persistDockLayout\(api\)\s+dockInitializedRef\.current = false\s+window\.clearTimeout/,
  )
  assert.match(
    dockHook,
    /if \(!dockInitializedRef\.current \|\| dockApiRef\.current !== api\) return/,
  )
  assert.match(
    dockHook,
    /api\.onDidActivePanelChange\(\(\{ panel \}\) => \{[\s\S]*?scheduleDockLayoutSave\(api\)/,
  )
})

test('initial dock sessions prefer the active chat and otherwise use the first session', () => {
  assert.deepEqual(
    initialDockSessionIds({
      activeSessionId: 'b',
      validSessionIds: ['a', 'b', 'c'],
    }),
    ['b'],
  )
  assert.deepEqual(
    initialDockSessionIds({
      activeSessionId: 'missing',
      validSessionIds: ['first', 'second'],
    }),
    ['first'],
  )
  assert.deepEqual(initialDockSessionIds(), [])
})
