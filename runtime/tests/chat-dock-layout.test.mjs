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

test('chat split controls expose left, right, top and bottom actions', async () => {
  const [dock, focus, dockHook, history, page, catalog, app, header] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/use-chat-dock.ts', 'utf8'),
    readFile('src/features/chat/ChatHistoryPage.tsx', 'utf8'),
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/layout/PageHeader.tsx', 'utf8'),
  ])
  assert.match(dock, /splitDockPanel\(panelId, 'above'\)/)
  assert.match(dock, /splitDockPanel\(panelId, 'below'\)/)
  assert.match(focus, /onSplitTop/)
  assert.match(focus, /onSplitBottom/)
  assert.match(dockHook, /splitDockPanel\(panel\.id, 'above'\)/)
  assert.match(dockHook, /splitDockPanel\(panel\.id, 'below'\)/)
  assert.match(history, /openSession\(session\.id, 'above'\)/)
  assert.match(history, /openSession\(session\.id, 'below'\)/)
  assert.match(dock, /role="tablist"/)
  assert.match(dock, /role="tab"/)
  assert.match(dock, /<nav[\s\S]*role="tablist"/)
  assert.match(dock, /onCreateSession/)
  assert.match(dock, /closeMobileSessionTab/)
  assert.match(dock, /chat:chatPage\.closeChat/)
  assert.match(page, /sessionIds=\{dock\.mobileSessionIds\}/)
  assert.match(page, /mobile_resume_local_runtime/)
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
  assert.match(page, /await invokeMobile<void>\('mobile_resume_local_runtime'\)/)
  assert.match(page, /refreshSessions\(undefined, \{ preserveExistingOnEmpty: true \}\)/)
  assert.match(catalog, /preserveExistingOnEmpty = false/)
  assert.match(catalog, /!data\.sessions\.length && sessionsRef\.current\.length/)
  assert.match(shell, /async fn mobile_resume_local_runtime/)
  assert.match(shell, /invalidate_remote_upstream/)
  assert.match(shell, /ensure_local_runtime_ready\(state\.on_device\.clone\(\)/)
  assert.match(
    shell,
    /RunEvent::Resumed[\s\S]*invalidate_remote_upstream[\s\S]*ensure_local_runtime_ready/,
  )
})

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
