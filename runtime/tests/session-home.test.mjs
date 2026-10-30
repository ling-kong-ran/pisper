// 会话做主页（负责人决定，取代“看板是首页”）：主页落地判定、“上次停留的会话”偏好、
// 一级标签归属（会话 / 看板 / 组件 / 更多 / 设置，手机没有组件标签）与原生返回的根。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { BROWSER_PREFERENCE_KEYS } from '../../shared/browser-preferences.mjs'
import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_ACTIONS,
  findShortcutConflict,
  normalizeShortcutBindings,
  validateShortcutBindings,
} from '../../shared/shortcuts.mjs'
import {
  HOME_ENTRY_STATE,
  HOME_PATH,
  isHomeEntry,
  legacyHashPath,
  pageFromPath,
  pagePath,
  sessionPath,
} from '../../src/app/routes.ts'
import {
  PHONE_SHELL_TABS,
  SHELL_TABS,
  activeShellTab,
  shellBackPath,
} from '../../src/app/shell-tabs.ts'
import { ACTIVITY_NAVIGATION_STATE, settingsOwnsPage } from '../../src/app/settings-navigation.ts'
import { getMoreNavigation } from '../../src/app/more-navigation.ts'
import { getNavigation } from '../../src/app/palette-navigation.ts'
import { AppTitlebar } from '../../src/app/AppTitlebar.tsx'

const t = (key) => key

test('the home route is the session page: / belongs to the session tab and unknown ids fall back to it', () => {
  assert.equal(HOME_PATH, '/')
  assert.equal(pageFromPath('/'), 'session')
  assert.equal(pageFromPath(''), null)
  // 会话路径本期不改名：/board/:id 仍是会话，/board 仍是看板。
  assert.equal(pageFromPath('/board'), 'board')
  assert.equal(pageFromPath('/board/abc'), 'session')
  assert.equal(pagePath('unknown'), HOME_PATH)
  assert.equal(pagePath('board'), '/board')
  // ⌘N / “新任务”与 /board/new 的进入即新建不变；主页不是它。
  assert.equal(pagePath('session'), '/board/new')
  assert.equal(sessionPath(), '/board/new')
  // 旧的 #chat 仍经 /chat 兼容入口（路由层重定向到主页），#session 不能借启动悄悄建会话。
  assert.equal(legacyHashPath('#chat'), '/chat')
  assert.equal(legacyHashPath('#session'), null)
  // 主页落地标记与“从更多进入”共用 navOrigin 字段，二者互斥。
  assert.equal(isHomeEntry(HOME_ENTRY_STATE), true)
  assert.equal(isHomeEntry({ navOrigin: 'home' }), true)
  assert.equal(isHomeEntry(ACTIVITY_NAVIGATION_STATE), false)
  for (const value of [null, undefined, 'home', 1, {}, { navOrigin: 'HOME' }])
    assert.equal(isHomeEntry(value), false)
})

test('router: index renders the home landing, unknown paths and /chat redirect home, /board stays', async () => {
  const router = await readFile('src/app/router.tsx', 'utf8')
  assert.match(router, /\{ index: true, lazy: homeRoute \}/)
  assert.match(
    router,
    /\{ path: PAGE_PATHS\.chat, element: <Navigate to=\{HOME_PATH\} replace \/> \}/,
  )
  assert.match(router, /\{ path: '\*', element: <Navigate to=\{HOME_PATH\} replace \/> \}/)
  assert.match(router, /\{ path: PAGE_PATHS\.board, lazy: boardRoute \}/)
  assert.match(router, /\{ path: '\/board\/:sessionId', lazy: sessionRoute \}/)
  assert.doesNotMatch(router, /<Navigate to=\{PAGE_PATHS\.board\}/)
  // 主页落地与草稿随会话路由懒加载，不进入入口包。
  const loaders = await readFile('src/app/route-elements.tsx', 'utf8')
  const home = loaders.slice(
    loaders.indexOf('export async function homeRoute'),
    loaders.indexOf('export async function moreRoute'),
  )
  assert.match(home, /import\('@\/app\/SessionHome'\)/)
  assert.match(home, /ensureBoardMessages\(\)/)
  const app = await readFile('src/App.tsx', 'utf8')
  assert.doesNotMatch(app, /SessionHome|session-home|NewTaskDraft/)
})

test('the home landing never creates a session; the draft creates one only on the first message', async () => {
  const [home, draft, shell] = await Promise.all([
    readFile('src/app/SessionHome.tsx', 'utf8'),
    readFile('src/features/board/NewTaskDraft.tsx', 'utf8'),
    readFile('src/app/SessionShell.tsx', 'utf8'),
  ])
  for (const source of [home, draft]) {
    assert.doesNotMatch(
      source,
      /createSession|requestSessionCreation|sessionPath\(\)|NEW_SESSION_ROUTE_ID/,
    )
    assert.doesNotMatch(source, /chatApi|apiJson|\bfetch\(/)
  }
  // 草稿复用启动台：发出第一条消息才走 launchBoardTask（建会话 + 首条消息）。
  assert.match(draft, /<LaunchConsole\s+variant="composer"/)
  // 落地、草稿发出与主页里的泳道条切换都替换当前条目并标为主页落地。
  assert.match(home, /navigate\(sessionPath\(id\), \{ replace: true, state: HOME_ENTRY_STATE \}\)/)
  assert.match(home, /onDispatched=\{openHomeSession\}/)
  // 目录读取失败给出错误态与重试，不静默回落；明确选择后才进草稿。
  assert.match(home, /kind="error"/)
  assert.match(home, /sessions\.refetch\(\)/)
  // 会话页：只记目录里确实存在的会话；归档后清掉记录回主页；切换代理保留主页落地标记。
  assert.match(shell, /if \(known\) rememberLastSession\(sessionId\)/)
  assert.match(shell, /forgetLastSession\(id\)\s+navigate\(HOME_PATH, \{ replace: true \}\)/)
  assert.match(
    shell,
    /navigate\(sessionPath\(id\), \{ replace: true, state: entryStateRef\.current \}\)/,
  )
  assert.match(shell, /<BoardEntry onOpenBoard=\{openBoard\} \/>/)
  assert.doesNotMatch(shell, /SessionReturnBar/)
})

test('home landing: last session if it still exists, draft otherwise, wait while loading, error on failure', async () => {
  const { homeLanding } = await import('../../src/app/session-home.ts')
  const data = {
    sessions: [{ id: 'a', archived: false }, { id: 'b', archived: true }, { id: 42 }, null, 'junk'],
  }
  assert.deepEqual(homeLanding({ lastId: '', data: undefined, failed: false }), { kind: 'draft' })
  assert.deepEqual(homeLanding({ lastId: '', data: undefined, failed: true }), { kind: 'draft' })
  assert.deepEqual(homeLanding({ lastId: 'a', data: undefined, failed: false }), {
    kind: 'loading',
  })
  assert.deepEqual(homeLanding({ lastId: 'a', data: { nope: 1 }, failed: false }), {
    kind: 'loading',
  })
  assert.deepEqual(homeLanding({ lastId: 'a', data: undefined, failed: true }), { kind: 'error' })
  // 有失败前的旧快照时仍按快照判断，不报错。
  assert.deepEqual(homeLanding({ lastId: 'a', data, failed: true }), { kind: 'session', id: 'a' })
  assert.deepEqual(homeLanding({ lastId: 'a', data, failed: false }), { kind: 'session', id: 'a' })
  // 已归档、已删除（不在目录里）→ 新任务草稿。
  assert.deepEqual(homeLanding({ lastId: 'b', data, failed: false }), { kind: 'draft' })
  assert.deepEqual(homeLanding({ lastId: 'gone', data, failed: false }), { kind: 'draft' })
  assert.deepEqual(homeLanding({ lastId: 'a', data: { sessions: [] }, failed: false }), {
    kind: 'draft',
  })
})

test('last-session preference is per device, validated as untrusted input and only goes through page-state storage', async () => {
  assert.ok(BROWSER_PREFERENCE_KEYS.includes('pisper-last-session'))
  const store = new Map()
  const storage = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: (key) => store.delete(key),
  }
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { localStorage: storage, sessionStorage: storage },
  })
  try {
    const {
      LAST_SESSION_ID_MAX_LENGTH,
      LAST_SESSION_KEY,
      forgetLastSession,
      parseLastSessionId,
      readLastSession,
      rememberLastSession,
    } = await import('../../src/app/session-home.ts')
    assert.equal(LAST_SESSION_KEY, 'pisper-last-session')
    assert.equal(parseLastSessionId('  abc  '), 'abc')
    for (const value of [null, undefined, 1, {}, '', '   ', 'new', 'a\u0000b', 'a\nb'])
      assert.equal(parseLastSessionId(value), '', JSON.stringify(value))
    assert.equal(parseLastSessionId('x'.repeat(LAST_SESSION_ID_MAX_LENGTH)).length, 256)
    assert.equal(parseLastSessionId('x'.repeat(LAST_SESSION_ID_MAX_LENGTH + 1)), '')

    assert.equal(readLastSession(), '')
    rememberLastSession('session-1')
    assert.equal(store.get(LAST_SESSION_KEY), 'session-1')
    assert.equal(readLastSession(), 'session-1')
    // 'new' 不是真实会话，非法值不覆盖已有记录。
    rememberLastSession('new')
    rememberLastSession('bad\u0007id')
    assert.equal(readLastSession(), 'session-1')
    // 只清掉指向被归档会话的记录。
    forgetLastSession('other')
    assert.equal(readLastSession(), 'session-1')
    forgetLastSession('session-1')
    assert.equal(readLastSession(), '')
    // 存储里被改坏的值读出来是空，不拿去拼路由。
    store.set(LAST_SESSION_KEY, 'x'.repeat(300))
    assert.equal(readLastSession(), '')
  } finally {
    if (previous) Object.defineProperty(globalThis, 'window', previous)
    else delete globalThis.window
  }
  const source = await readFile('src/app/session-home.ts', 'utf8')
  assert.match(source, /from '@\/lib\/page-state-storage'/)
  assert.doesNotMatch(source, /localStorage/)
})

test('primary tabs: session first; the session page and home light "session", the board lights "board"', () => {
  assert.deepEqual([...SHELL_TABS], ['session', 'board', 'components', 'more', 'settings'])
  // 手机标签栏：会话 · 看板 · [新任务] · 更多 · 设置；组件挪进“更多”。
  assert.deepEqual([...PHONE_SHELL_TABS], ['session', 'board', 'more', 'settings'])
  const tab = (path, state, phone = false) => {
    const page = pageFromPath(path)
    return activeShellTab(page, settingsOwnsPage(page, state), phone)
  }
  assert.equal(tab('/'), 'session')
  assert.equal(tab('/board/abc'), 'session')
  assert.equal(tab('/board/new'), 'session')
  assert.equal(tab('/chat'), 'session')
  assert.equal(tab('/board'), 'board')
  assert.equal(tab('/components'), 'components')
  assert.equal(tab('/components/x'), 'components')
  // 手机没有组件标签：从更多进入组件时点亮“更多”。
  assert.equal(tab('/components', ACTIVITY_NAVIGATION_STATE, true), 'more')
  assert.equal(tab('/components/x', undefined, true), 'more')
  assert.equal(tab('/more', undefined, true), 'more')
  assert.equal(tab('/assets'), 'more')
  assert.equal(tab('/config/models'), 'settings')
  assert.equal(tab('/plugins'), 'settings')
  assert.equal(tab('/plugins', ACTIVITY_NAVIGATION_STATE), 'more')
})

test('native back: the session home is the root, opened sessions go back where they came from, top-level pages go home', () => {
  const back = (path, { state, phone = false, canGoBack = false } = {}) => {
    const page = pageFromPath(path)
    return shellBackPath(page, path, settingsOwnsPage(page, state), {
      phone,
      homeEntry: isHomeEntry(state),
      canGoBack,
    })
  }
  // 主页（草稿）与从主页落地的会话是根：交还平台。
  assert.equal(back('/'), null)
  assert.equal(back('/', { canGoBack: true }), null)
  assert.equal(back('/board/abc', { state: HOME_ENTRY_STATE, canGoBack: true }), null)
  // 从看板等页面点进来的会话：走历史回到来处；冷启动直接打开的会话链接没有来处，交还平台。
  assert.equal(back('/board/abc', { canGoBack: true }), -1)
  assert.equal(back('/board/abc', { canGoBack: false }), null)
  assert.equal(back('/board/new', { canGoBack: true }), -1)
  // 一级页回主页。
  for (const path of ['/board', '/more', '/config/models', '/config/interface', '/plugins'])
    assert.equal(back(path), HOME_PATH, path)
  assert.equal(back('/components'), HOME_PATH)
  // 更多的子页回更多；组件详情回列表；手机上组件列表回更多。
  assert.equal(back('/assets'), '/more')
  assert.equal(back('/chat/history'), '/more')
  assert.equal(back('/plugins', { state: ACTIVITY_NAVIGATION_STATE }), '/more')
  assert.equal(back('/tools/components/game-assets'), '/more')
  assert.equal(back('/components/game-assets'), '/components')
  assert.equal(back('/components/game-assets', { phone: true }), '/components')
  assert.equal(back('/components', { phone: true }), '/more')
  assert.equal(back('/components', { state: ACTIVITY_NAVIGATION_STATE, phone: true }), '/more')
})

test('the shell wires the session tab to the home, back to history, and the board entry to its own shortcut', async () => {
  const app = await readFile('src/App.tsx', 'utf8')
  // “会话”标签按路由器的最新去向判断（规则见 shell-tabs.sessionTabNavigation 的单元测试），
  // 不看已渲染的 page：刚离开主页、界面还没换过去时 page 仍是 session。
  assert.match(
    app,
    /if \(tab === 'session'\) \{\s*const next = sessionTabNavigation\(readRouter\(\)\)\s+if \(next\) routerNavigate\(next\.to, \{ replace: next\.replace, state: next\.state \}\)/,
  )
  assert.match(app, /routerState\?\.\(\) \?\? \{ location, navigation: \{ state: 'idle' \} \}/)
  const router = await readFile('src/app/router.tsx', 'utf8')
  assert.match(router, /const routerState = \(\) => router\.state/)
  assert.match(router, /<App routerState=\{routerState\} \/>/)
  assert.match(app, /const shellTab = activeShellTab\(page, settingsOwned, phoneShell\)/)
  assert.match(app, /homeEntry: isHomeEntry\(location\.state\)/)
  assert.match(app, /if \(parent === -1\) routerNavigate\(-1\)/)
  assert.match(app, /onShowBoard=\{page === 'session' \? backToBoard : undefined\}/)
  // ⌘1（openSession）与点“会话”标签同一个入口：主页逻辑，从不建会话；⌘[ 的返回看板已不再接线。
  assert.match(app, /onOpenSession=\{\(\) => selectShellTab\('session'\)\}/)
  assert.doesNotMatch(app, /onBackToBoard/)
  // 兼容垫片 navigate('chat') 没有待打开的会话时回主页；Runtime 裁掉的页面回主页。
  assert.match(app, /\? sessionPath\(intent\.sessionId\)\s+: HOME_PATH,/)
  assert.match(app, /routerNavigate\(HOME_PATH, \{ replace: true \}\)/)
  // 键盘 Esc 仍只走返回层；路由层返回只挂在手机原生返回上。
  assert.match(app, /<BackNavigation onRouteBack=\{mobileLayout \? routeBack : undefined\} \/>/)
  const entry = await readFile('src/app/BoardEntry.tsx', 'utf8')
  assert.doesNotMatch(entry, /keydown|Escape|onKeyDown/)
  assert.match(entry, /state\.bindings\.showBoard/)
})

test('on the home page the shell navigates itself: no session route is mounted there to take requests', async () => {
  const [app, preview, home, launch] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/app/WebPreviewProvider.tsx', 'utf8'),
    readFile('src/app/SessionHome.tsx', 'utf8'),
    readFile('src/features/board/LaunchConsole.tsx', 'utf8'),
  ])
  // '/' 的页面 id 是 'session'，挂的却是 SessionHome：判断“会话路由已挂载”只能看 /board/:id。
  assert.match(
    app,
    /const sessionRouteMounted = Boolean\(sessionIdFromPath\(location\.pathname\)\)/,
  )
  const body = (name) => {
    const start = app.indexOf(`const ${name} = useCallback(`)
    assert.ok(start > 0, name)
    return app.slice(start, app.indexOf('\n  )', start))
  }
  // 打开会话（⌘K、舰队计量、⌥A、派发浮层 ⌘↵）：主页上外壳必须自己导航过去。
  assert.match(
    body('openSession'),
    /if \(!sessionRouteMounted\) routerNavigate\(sessionPath\(id\)\)/,
  )
  // 兼容垫片 navigate('chat') 与新会话：同样按“会话路由是否挂载”决定，而不是 page === 'session'。
  assert.match(
    body('navigate'),
    /if \(!sessionRouteMounted\) \{\s*const intent = peekSessionNavigationIntent\(\)/,
  )
  assert.match(
    app,
    /if \(sessionRouteMounted\) requestSessionCreation\(''\)\s+else routerNavigate\(sessionPath\(\)\)/,
  )
  for (const name of ['openSession', 'navigate'])
    assert.doesNotMatch(body(name), /page !== 'session'|page === 'session'/, name)
  // 外链预览：只有会话路由挂着检查器，主页按“不在会话页”处理（带请求跳到最近的会话）。
  assert.match(
    preview,
    /if \(sessionIdFromPath\(location\.pathname\)\) \{\s*requestWebPreview\(url\)/,
  )
  assert.doesNotMatch(preview, /pageFromPath/)
  // 草稿第一条消息还在发送时离开了主页：不再 replace 别人的页面，改为提示并给出打开入口。
  assert.match(
    home,
    /if \(mountedRef\.current\) \{\s*navigate\(sessionPath\(id\), \{ replace: true, state: HOME_ENTRY_STATE \}\)/,
  )
  assert.match(home, /onAction: \(\) => navigate\(sessionPath\(id\)\)/)
  // 草稿输入框的发送键跟随“设置 → 快捷键”的 sendMessage（与会话页输入框一致），占位文字随绑定。
  assert.match(
    launch,
    /if \(composer\) \{\s*if \(matchesShortcut\(event\.nativeEvent, sendBinding\)\)/,
  )
  assert.match(launch, /t\('board:home\.placeholder', \{ send: formatShortcut\(sendBinding\) \}\)/)
  assert.match(launch, /t\('board:home\.placeholderUnbound'\)/)
})

test('"open board" is its own rebindable shortcut; ⌘B stays with the hidden sidebar toggle', async () => {
  // showBoard 之后只追加了“会话”标签动作（openSession）。
  assert.deepEqual(SHORTCUT_ACTIONS.slice(-2), ['showBoard', 'openSession'])
  assert.equal(DEFAULT_SHORTCUTS.showBoard, 'Mod+Shift+KeyB')
  // ⌘B 是隐藏动作 toggleSidebar 的默认键：同一张表不能重复，因此默认改用 ⇧⌘B。
  assert.equal(DEFAULT_SHORTCUTS.toggleSidebar, 'Mod+KeyB')
  assert.equal(findShortcutConflict(DEFAULT_SHORTCUTS, 'showBoard', 'Mod+KeyB'), 'toggleSidebar')
  assert.equal(findShortcutConflict(DEFAULT_SHORTCUTS, 'showBoard', 'Mod+Shift+KeyB'), null)
  assert.deepEqual(validateShortcutBindings(DEFAULT_SHORTCUTS), DEFAULT_SHORTCUTS)
  // 旧版保存的表补上新动作；默认键已被用户占用时补为未绑定，自定义原样保留。
  const saved = Object.fromEntries(
    SHORTCUT_ACTIONS.filter((action) => action !== 'showBoard').map((action) => [
      action,
      DEFAULT_SHORTCUTS[action],
    ]),
  )
  assert.equal(normalizeShortcutBindings(saved).showBoard, 'Mod+Shift+KeyB')
  const taken = normalizeShortcutBindings({ ...saved, commandPalette: 'Mod+Shift+KeyB' })
  assert.equal(taken.commandPalette, 'Mod+Shift+KeyB')
  assert.equal(taken.showBoard, null)
  const [catalog, shortcuts] = await Promise.all([
    readFile('src/features/config/shortcut-catalog.ts', 'utf8'),
    readFile('src/components/layout/AppShortcuts.tsx', 'utf8'),
  ])
  // 会话组的第一行是“打开看板”；⌘[ 的“返回看板”已退役，不再列出。
  assert.match(
    catalog,
    /id: 'session',\s+entries: \[\s+app\('showBoard'\),\s+app\('previousAgent'\)/,
  )
  assert.doesNotMatch(catalog, /backToBoard/)
  assert.match(catalog, /showBoard: t\('board:shortcuts\.showBoard'\)/)
  assert.match(shortcuts, /matchesShortcut\(event, bindings\.showBoard\)\) action = onShowBoard/)
})

test('new-task requests on the home draft focus the draft instead of stacking the dispatch overlay', async () => {
  const [app, home, draft, launch] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/app/SessionHome.tsx', 'utf8'),
    readFile('src/features/board/NewTaskDraft.tsx', 'utf8'),
    readFile('src/features/board/LaunchConsole.tsx', 'utf8'),
  ])
  // 按键只有 AppShortcuts 一个入口（primaryAction 绑定，改绑照常生效），⌘N 与标题栏按钮都交给 startNewTask。
  assert.match(app, /onPrimary=\{\(\) => startNewTask\('shortcut'\)\}/)
  assert.match(app, /onNewTask=\{\(\) => startNewTask\('titlebar'\)\}/)
  // 主页的页面 id 也是 session：先问当前页有没有认领者，没有才打开派发浮层。
  assert.match(
    app,
    /if \(requestedPathname\(readRouter\(\)\) !== location\.pathname\) \{\s+pendingNewTask\.current = source\s+return\s+\}\s+if \(\(page === 'board' \|\| page === 'session'\) && requestBoardNewTask\(source\)\) return\s+setDispatchOpen\(true\)/,
  )
  // 路由器已请求去别处、渲染的 page 还是离开中的那页时，请求先记下，目标页提交后再问一次（只执行一次）。
  assert.match(
    app,
    /const source = pendingNewTask\.current\s+if \(!source \|\| requestedPathname\(readRouter\(\)\) !== location\.pathname\) return\s+pendingNewTask\.current = null\s+startNewTask\(source\)/,
  )
  // 认领者只有新任务草稿本身：主页只在落到草稿时挂它，加载中 / 错误态与真实会话页没有认领者。
  assert.match(home, /if \(landing\.kind === 'draft'\)\s+body = \(\s+<NewTaskDraft/)
  // 只认领 ⌘N 与标着同一个 ⌘N 的标题栏按钮；命令面板的“新会话”照旧开浮层（面板关闭会把焦点还回去）。
  assert.match(
    draft,
    /DRAFT_CLAIMED_SOURCES: ReadonlySet<BoardNewTaskSource> = new Set\(\['shortcut', 'titlebar'\]\)/,
  )
  assert.match(
    draft,
    /useBoardNewTaskHandler\(\(\{ source \}\) => \{\s+if \(!DRAFT_CLAIMED_SOURCES\.has\(source\) \|\| !consoleRef\.current\) return false\s+consoleRef\.current\.focusEnd\(\)\s+return true\s+\}\)/,
  )
  assert.match(draft, /variant="composer"\s+ref=\{consoleRef\}/)
  // 草稿不自己听键盘，不出现第二个 ⌘N 监听。
  assert.doesNotMatch(draft, /keydown|addEventListener\(|matchesShortcut|bindings\.primaryAction/)
  // 聚焦时光标放到末尾，接着已写的内容往下写；设置选区不会滚动，长草稿要手动滚到底才看得到光标。
  assert.match(
    launch,
    /focusEnd: \(\) => \{\s+const input = inputRef\.current\s+if \(!input\) return\s+input\.focus\(\)\s+input\.setSelectionRange\(input\.value\.length, input\.value\.length\)\s+(?:\/\/[^\n]*\s+)?input\.scrollTop = input\.scrollHeight\s+\}/,
  )
})

test('title bar renders five tabs in order and lights the session tab on the home', () => {
  const html = renderToStaticMarkup(
    h(AppTitlebar, {
      activeTab: 'session',
      onSelectTab() {},
      onOpenPalette() {},
      onNewTask() {},
      fleet: h('span', { 'data-fleet-slot': '' }),
    }),
  )
  const tabs = [...html.matchAll(/data-shell-tab="(\w+)"/g)].map((match) => match[1])
  assert.deepEqual(tabs, ['session', 'board', 'components', 'more', 'settings'])
  assert.deepEqual(
    [...html.matchAll(/<button[^>]*aria-current="page"[^>]*data-shell-tab="(\w+)"/g)].map(
      (match) => match[1],
    ),
    ['session'],
  )
  assert.match(html, /lucide-session-bubble/)
  // 五个标签都在提示里标出自己的快捷键（⌘1–⌘4 与 ⌘,），“会话”也不例外。
  assert.deepEqual(
    [...html.matchAll(/title="[^"]*\((Ctrl|Cmd) \+ ([^)]+)\)"[^>]*data-shell-tab="(\w+)"/g)].map(
      (match) => `${match[3]}:${match[2]}`,
    ),
    ['session:1', 'board:2', 'components:3', 'more:4', 'settings:,'],
  )
})

test('phone More lists the components page as a tile; desktop keeps it as a primary tab', () => {
  const group = (options) =>
    getMoreNavigation(t, options).find((item) => item.id === 'components')?.tiles || []
  assert.equal(
    group({}).some((tile) => tile.key === 'shell:components'),
    false,
  )
  const phone = group({ componentsTile: true })
  assert.equal(phone[0].key, 'shell:components')
  assert.deepEqual(phone[0].destination, { type: 'page', id: 'components' })
  // 最后一格仍是“+ 添加”。
  assert.equal(phone.at(-1).key, 'page:components')
  // 命令面板：“会话”用兼容页面 id 'chat'（navigate('chat') 落到主页，不建会话）。
  const [[, shell]] = getNavigation(t)
  assert.deepEqual(
    shell.map(([page]) => page),
    ['chat', 'board', 'components', 'more', 'config'],
  )
})
