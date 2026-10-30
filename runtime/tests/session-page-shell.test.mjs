// 会话页外壳（规格 v2 第 4.1 / 4.4 / 4.6 节）：并行的代理在应用侧栏里，会话页没有泳道条与 ⌘J 代理面板；
// 检查器偏好，以及“critical 审批在任何入口都不能一步批准”在侧栏、手机代理条与会话审批面板上的源码守卫。
// 第 3 期的泳道条收缩顺序与迷你卡片守卫随泳道条退役（A1）；等价的保护改为侧栏分组模型
// （sidebar-model.test.mjs：长列表规则、只有完整的普通批准才能一步处理、⌥↑⌥↓ 顺序）与下面的源码守卫。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import test from 'node:test'
import { effectiveStripCurrent, stripOrder } from '../../src/features/board/strip-layout.ts'
import { requestedPathname } from '../../src/app/shell-tabs.ts'
import { sessionIdFromPath } from '../../src/app/routes.ts'

const root = resolve(import.meta.dirname, '../..')
const read = (path) => readFile(resolve(root, path), 'utf8')
test('the phone agent strip keeps the lane order of the retired desktop strip', () => {
  // 手机代理条（A4 前）沿用需要你 → 运行中 → 排队 → 已完成的顺序；桌面的切换顺序改为侧栏可见顺序。
  const order = stripOrder([
    { id: 'd1', lane: 'done' },
    { id: 'r1', lane: 'running' },
    { id: 'a1', lane: 'attention' },
    { id: 'q1', lane: 'queued' },
    { id: 'a2', lane: 'attention' },
  ])
  assert.deepEqual(
    order.map((item) => item.id),
    ['a1', 'a2', 'r1', 'q1', 'd1'],
  )
})

// 路由在 transition 里提交：⌥↓ 之后 URL 已换、界面还停在原会话时立刻再按 ⌥↑，
// 必须以刚发出的目标为起点，回到原会话，而不是从原会话再算一次邻居（smoke-board-ui 7a 曾偶发失败）。
test('rapid option-arrow presses start from the switch that has not committed yet', async () => {
  // 手机代理条：界面仍停在 r2、已发出去 r3 的切换时，以 r3 为起点。
  const pending = { from: 'r2', to: 'r3' }
  assert.equal(effectiveStripCurrent('r2', pending), 'r3')
  assert.equal(effectiveStripCurrent('r3', pending), 'r3')
  assert.equal(effectiveStripCurrent('a1', pending), 'a1')
  assert.equal(effectiveStripCurrent('r2', null), 'r2')
  // 桌面侧栏：当前会话读路由器的最新去向（navigate 同步改它），⌥↓ 之后 URL 已换、界面还停在原会话时
  // 立刻再按 ⌥↑，起点已经是刚发出的目标（smoke-board-ui 7a 曾因只看渲染状态偶发失败）。
  const loading = {
    location: { pathname: '/board/r2' },
    navigation: { state: 'loading', location: { pathname: '/board/r3' } },
  }
  assert.equal(sessionIdFromPath(requestedPathname(loading)), 'r3')
  const [shell, app, sidebar] = await Promise.all([
    read('src/app/SessionShell.tsx'),
    read('src/App.tsx'),
    read('src/features/board/SidebarSessions.tsx'),
  ])
  assert.match(shell, /pendingSwitchRef\.current = \{ from, to: id \}/)
  assert.match(shell, /id === effectiveStripCurrent\(from, pendingSwitchRef\.current\)/)
  assert.match(
    app,
    /const currentSessionTarget = useCallback\(\s*\(\) => sessionIdFromPath\(requestedPathname\(readRouter\(\)\)\),/,
  )
  assert.match(app, /if \(sessionIdFromPath\(target\) === id\) return/)
  assert.match(app, /<SidebarCommands\s+resolveCurrentId=\{currentSessionTarget\}/)
  assert.match(
    sidebar,
    /adjacentSidebarSession\(\s*current\.order,\s*propsRef\.current\.resolveCurrentId\(\) \|\| null,/,
  )
})

// 规格 v2 第 2.2 / 2.3 / 2.8 节的键盘与读屏接线（A1 键盘段）；行为由 smoke-sidebar 在浏览器里逐项验证。
test('sidebar keyboard wiring: F6 regions, A / D on rows, return focus and the polite announcement', async () => {
  const [app, sidebar, regions, shell, header, focusSession, draft] = await Promise.all([
    read('src/App.tsx'),
    read('src/features/board/SidebarSessions.tsx'),
    read('src/app/focus-regions.ts'),
    read('src/app/ShellSidebar.tsx'),
    read('src/features/chat/SessionPageHeader.tsx'),
    read('src/features/chat/FocusSession.tsx'),
    read('src/features/board/NewTaskDraft.tsx'),
  ])
  // F6：外壳只在按下时动态加载区域逻辑（不进入口包）；区域顺序 侧栏 → 顶栏 → 对话 / 内容 → 输入框。
  assert.match(
    app,
    /onCycleFocusRegion=\{\(\) =>\s+void import\('@\/app\/focus-regions'\)\.then\(\(module\) => module\.cycleFocusRegion\(\)\)/,
  )
  assert.doesNotMatch(app, /^import .*focus-regions/m)
  assert.match(
    regions,
    /const RANK: Record<RegionKind, number> = \{ sidebar: 0, header: 1, content: 2, main: 2, composer: 3 \}/,
  )
  assert.match(regions, /\[data-sidebar-row\]\[aria-current="page"\]/)
  assert.match(shell, /data-focus-region="sidebar"/)
  assert.match(app, /<main\s+data-focus-region="main"/)
  assert.match(header, /data-session-header\s+data-focus-region="header"/)
  assert.match(focusSession, /data-chat-region="messages"\s+data-focus-region="content"/)
  assert.match(focusSession, /data-chat-region="composer" data-focus-region="composer"/)
  assert.equal(draft.match(/data-focus-region="composer"/g)?.length, 2)
  // 侧栏行的 A / D 跟随全部代理的单字母绑定，必须用看板匹配器（应用匹配器拒绝无修饰的字母键，
  // 7c3052bb 用错匹配器导致 A / D 在侧栏上从未生效）。
  assert.match(sidebar, /matchesBoardShortcut\(event\.nativeEvent, boardBindings\.boardApprove\)/)
  assert.match(sidebar, /matchesBoardShortcut\(event\.nativeEvent, boardBindings\.boardDeny\)/)
  assert.doesNotMatch(sidebar, /matchesShortcut\(event\.nativeEvent, boardBindings/)
  // 返回时焦点回到原会话行：外壳把已渲染的会话交给常驻的 SidebarCommands，只在焦点无主时移动。
  assert.match(
    app,
    /renderedSessionId=\{\s*page === 'session' \? sessionIdFromPath\(location\.pathname\) : null\s*\}/,
  )
  assert.match(
    sidebar,
    /if \(!left \|\| left !== renderedSessionId \|\| useSidebarStore\.getState\(\)\.collapsed\) return/,
  )
  assert.match(sidebar, /\{\s*unclaimed: true,?\s*\}/)
  // 需要你的 polite 播报在常驻的 SidebarCommands 里（窄轨时分组不挂载也要播），分组里不再重复一份。
  const commands = sidebar.slice(sidebar.indexOf('export function SidebarCommands'))
  const commandsBody = commands.slice(0, commands.indexOf('export type SidebarRailCountsProps'))
  assert.match(
    commandsBody,
    /<p data-sidebar-announce aria-live="polite" role="status" className="sr-only">/,
  )
  assert.match(commandsBody, /t\('board:sidebar\.announce'/)
  assert.equal(sidebar.match(/aria-live="polite"/g)?.length, 1)
})

test('inspector preferences persist open, tab and width under a registered key', async () => {
  const values = new Map([
    [
      'pisper-session-inspector',
      JSON.stringify({
        state: { open: true, tab: 'side-chat', width: 9999, terminalView: 'processes' },
        version: 1,
      }),
    ],
  ])
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  }
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { localStorage: storage },
  })
  try {
    const {
      SESSION_INSPECTOR_MAX_WIDTH,
      SESSION_INSPECTOR_MIN_WIDTH,
      inspectorDisplayWidth,
      normalizeInspectorPreferences,
      useSessionInspectorStore: store,
    } = await import('../../src/features/chat/session-inspector-store.ts')
    const { BROWSER_PREFERENCE_KEYS } = await import('../../shared/browser-preferences.mjs')
    assert.ok(BROWSER_PREFERENCE_KEYS.includes('pisper-session-inspector'))
    // 侧聊是临时标签，恢复时回到“改动”；宽度夹在允许范围内。
    assert.equal(store.getState().open, true)
    assert.equal(store.getState().tab, 'changes')
    assert.equal(store.getState().width, SESSION_INSPECTOR_MAX_WIDTH)
    assert.equal(store.getState().terminalView, 'processes')
    store.getState().setTab('preview')
    store.getState().setWidth(100)
    assert.equal(store.getState().width, SESSION_INSPECTOR_MIN_WIDTH)
    assert.equal(JSON.parse(values.get('pisper-session-inspector')).state.tab, 'preview')
    assert.equal(normalizeInspectorPreferences({ open: false }).open, false)
    assert.deepEqual(normalizeInspectorPreferences('broken'), {
      open: true,
      tab: 'changes',
      width: 456,
      terminalView: 'terminal',
      changesView: 'session',
      diffMode: 'unified',
    })
    // 差异视图只认 unified / split，其余值回到统一视图。
    assert.equal(normalizeInspectorPreferences({ diffMode: 'split' }).diffMode, 'split')
    assert.equal(normalizeInspectorPreferences({ diffMode: 'side' }).diffMode, 'unified')
    // 窗口变窄只收窄显示宽度，对话列至少保留 480px，不改写偏好。
    assert.equal(inspectorDisplayWidth(600, 1400), 600)
    assert.equal(inspectorDisplayWidth(600, 1000), 512)
    assert.equal(inspectorDisplayWidth(600, 700), SESSION_INSPECTOR_MIN_WIDTH)
  } finally {
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else delete globalThis.window
  }
})

test('session page layout: no lane strip above the chat, inspector overlay under 1280 or 760 tall', async () => {
  const [routes, workspace, header, focus, dock, page] = await Promise.all([
    read('src/app/SessionShell.tsx'),
    read('src/features/chat/SessionWorkspace.tsx'),
    read('src/features/chat/SessionPageHeader.tsx'),
    read('src/features/chat/FocusSession.tsx'),
    read('src/features/chat/ChatDock.tsx'),
    read('src/features/chat/ChatPage.tsx'),
  ])
  const session = routes
  assert.match(routes, /SESSION_COMPACT_QUERY = '\(max-width: 1279px\), \(max-height: 760px\)'/)
  // 手机 App 平板走桌面布局但检查器始终是覆盖层。规格 v2：桌面会话页不再有泳道条，并行的代理在
  // 应用侧栏里；手机外壳（A4 前）是同一份会话页契约加上看板领域的代理条（phoneChrome），看板入口在手机顶栏最左侧。
  assert.match(session, /const pageLayout = !phone \|\| tablet/)
  assert.match(session, /overlayOnly: mobileApp/)
  assert.doesNotMatch(session, /SessionLaneStrip|AgentPanel|resolveCurrentId/)
  assert.match(session, /pageChrome=\{pageLayout \? pageChrome : phoneChrome\}/)
  assert.match(
    session,
    /const phoneChrome = useMemo\(\s*\(\) => \(\{\s*\.\.\.pageChrome,\s*phone: \{\s*strip: \(\s*<PhoneAgentStrip currentId=\{sessionId\} notify=\{notify\} onOpenSession=\{switchSession\} \/>/,
  )
  assert.match(
    session,
    /routeLeading=\{pageLayout \? undefined : <BoardEntry onOpenBoard=\{openBoard\} \/>\}/,
  )
  // 切换代理替换历史记录：浏览器后退回到来处；从主页落地的会话保留主页标记（仍是返回的根）。
  assert.match(
    session,
    /navigate\(sessionPath\(id\), \{ replace: true, state: entryStateRef\.current \}\)/,
  )
  const loaders = await read('src/app/route-elements.tsx')
  const loader = loaders.slice(
    loaders.indexOf('export async function sessionRoute'),
    loaders.indexOf('export async function moreRoute'),
  )
  // 外壳编排按需加载，聊天页仍是独立的懒加载入口；看板文案随会话路由注册。
  assert.match(loader, /import\('@\/app\/SessionShell'\)/)
  assert.match(loader, /import\('@\/features\/chat\/ChatPage'\)/)
  assert.match(loader, /ensureBoardMessages\(\)/)
  // 页头 52px，⌘I 开关检查器；覆盖层 420px、无遮罩、--sh-float。
  // 规格 v2：主区顶栏 52（与侧栏品牌行对齐；A1 先改高度，顶栏内容在 A2 重做）。
  assert.match(header, /className="flex h-\[52px\] min-w-0 shrink-0/)
  assert.match(header, /data-inspector-toggle/)
  // 第 4 期起 ⌘I 是 shortcut-store 里的 toggleInspector（默认 Mod+KeyI，见 shortcuts.test），改绑后立即生效。
  assert.match(workspace, /useShortcutStore\(\(state\) => state\.bindings\.toggleInspector\)/)
  assert.match(workspace, /matchesShortcut\(event, inspectorBinding\)/)
  assert.match(workspace, /data-inspector-overlay[\s\S]{0,200}shadow-float/)
  assert.doesNotMatch(workspace, /overlay-scrim|backdrop/)
  assert.match(workspace, /useBackLayer\(active && overlayMode && overlayOpen, 100, close\)/)
  // 页头只在会话页布局里替换旧顶栏；旧顶栏（手机外壳）保持不变。
  assert.match(focus, /const headerBlock = pageHeaderBlock \|\| \(/)
  assert.match(dock, /pageChrome \? \(\s*\/\/[^\n]*\n\s*<SessionWorkspace/)
  assert.match(
    page,
    /SessionPageChromeContext\.Provider value=\{routeMode \? \(pageChrome \?\? null\) : null\}/,
  )
})

test('critical approvals cannot be approved in one step from the sidebar, the phone strip or the session', async () => {
  const [sidebar, phoneStrip, controller, toolApproval] = await Promise.all([
    read('src/features/board/SidebarSessions.tsx'),
    read('src/features/board/PhoneAgentStrip.tsx'),
    read('src/features/board/useBoardController.ts'),
    read('src/features/chat/ToolApproval.tsx'),
  ])
  // 侧栏快捷处理：只有 sidebar-model 判为 decide（完整详情、普通、无同时在等的问题）的行有“允许 / 拒绝”，
  // 且只传 confirmCritical: false；其余一律“打开”。需要确认时（详情变化）改为打开会话，不弹确认层也不放行。
  assert.doesNotMatch(sidebar, /confirmCritical: true/)
  assert.match(
    sidebar,
    /row\.quickAction\.kind === 'decide' \? \(\s*<>[\s\S]*?board:sidebar\.deny[\s\S]*?board:sidebar\.approve[\s\S]*?<\/>\s*\) : \(/,
  )
  assert.match(
    sidebar,
    /if \(result\.reason === 'confirmation-required'\) onOpenSession\(row\.id\)/,
  )
  // 手机代理条只把动作交回 useStripApprovals，不直接调批准接口。
  assert.doesNotMatch(phoneStrip, /approveBoardApproval|useBoardActions|confirmCritical/)
  assert.equal(phoneStrip.match(/confirmStripApproval\(\)/g)?.length, 1)
  assert.match(phoneStrip, /onConfirm=\{\(\) => void approvals\.confirmStripApproval\(\)\}/)
  const hook = controller.slice(controller.indexOf('export function useStripApprovals'))
  assert.match(
    hook,
    /if \(approval\.severity === 'critical' \|\| approval\.truncated\) \{\s*review\(item, approval\)\s*return/,
  )
  assert.equal(hook.match(/confirmCritical: true/g)?.length, 1)
  assert.match(
    hook.slice(hook.indexOf('const confirmStripApproval')),
    /^const confirmStripApproval = useCallback\(async \(\) => \{[\s\S]{0,400}confirmCritical: true/,
  )
  // 会话审批面板：critical 的“允许…”只打开确认层，确认层回调才真正批准。
  assert.match(toolApproval, /const critical = approvalSeverity\(approval\) === 'critical'/)
  assert.match(
    toolApproval,
    /onClick=\{\(\) => \(critical \? setConfirming\(true\) : resolve\(true\)\)\}/,
  )
  assert.match(
    toolApproval,
    /<ApprovalConfirm[\s\S]*?onConfirm=\{\(\) => \{\s*(?:\/\/[^\n]*\n\s*)?void resolve\(true\)/,
  )
})

test('the sidebar reads the board store and keeps escape for its own temporary expansion only', async () => {
  const [sidebar, shell, shortcuts] = await Promise.all([
    read('src/features/board/SidebarSessions.tsx'),
    read('src/app/ShellSidebar.tsx'),
    read('src/components/layout/AppShortcuts.tsx'),
  ])
  // ⌘J、⌥↑ / ⌥↓ 读 shortcut-store 的当前绑定（默认键见 shortcuts.test），可改绑；只换路由或移动焦点。
  assert.match(shortcuts, /matchesShortcut\(event, bindings\.agentPanel\)\) action = onAgentPanel/)
  assert.match(
    shortcuts,
    /matchesShortcut\(event, bindings\.previousAgent\)\) action = onPreviousAgent/,
  )
  assert.match(shortcuts, /matchesShortcut\(event, bindings\.nextAgent\)\) action = onNextAgent/)
  // 侧栏行的 A / D 跟随全部代理的改绑（boardBindings），用看板的单字母匹配器（见上面的接线守卫）。
  assert.match(sidebar, /matchesBoardShortcut\(event\.nativeEvent, boardBindings\.boardApprove\)/)
  assert.match(sidebar, /matchesBoardShortcut\(event\.nativeEvent, boardBindings\.boardDeny\)/)
  // Esc 只收回临时展开的侧栏，不离开页面。
  assert.match(shell, /if \(event\.key !== 'Escape'[\s\S]{0,120}setPeeking\(false\)/)
  assert.doesNotMatch(sidebar, /'Escape'/)
})

test('sidebar rows never set Chinese text in monospace', async () => {
  // 等宽只给机器输出（规格 6.3）：侧栏里唯一的等宽文字是运行中行的计时（数字）；原因、标题与时刻用无衬线。
  const sidebar = await read('src/features/board/SidebarSessions.tsx')
  assert.equal(sidebar.match(/font-mono/g)?.length, 1)
  assert.match(sidebar, /row\.group === 'running'\s*\? 'font-mono text-content-secondary'/)
  assert.match(sidebar, /meta = formatSidebarClock\(/)
})

test('the sidebar renders board store data and opens no connection of its own', async () => {
  const [sidebar, model, hook, presentation, shellSidebar, shell, items] = await Promise.all([
    read('src/features/board/SidebarSessions.tsx'),
    read('src/features/board/sidebar-model.ts'),
    read('src/features/board/useSidebarModel.ts'),
    read('src/features/board/sidebar-presentation.ts'),
    read('src/app/ShellSidebar.tsx'),
    read('src/app/SessionShell.tsx'),
    read('src/features/board/useBoardItems.ts'),
  ])
  const connection =
    /\bfetch\(|setInterval\(|EventSource|openBoardEvents|consumeEventStream|apiJson|useQuery\(|refetchInterval/
  for (const [file, source] of Object.entries({
    sidebar,
    model,
    hook,
    presentation,
    shellSidebar,
    shell,
  }))
    assert.doesNotMatch(source, connection, file)
  // 分组模型与文案推导只拿数据类型，不在运行时导入看板接口、store 或会话数据层。
  for (const [file, source] of Object.entries({ model, presentation })) {
    assert.doesNotMatch(
      source,
      /^import (?!type\b)[^\n]*from '\.\/(?:board-api|board-store|board-model|useBoardItems|useSessionListQuery)'/m,
      file,
    )
    assert.doesNotMatch(source, /from '@\/features\/chat\//, file)
  }
  // 唯一的数据源：useSidebarModel 读 useBoardItems('today')（看板 store 的同一份目录与覆盖层）。
  assert.equal(hook.match(/useBoardItems\(/g)?.length, 1)
  assert.match(hook, /useBoardItems\('today'\)/)
  assert.match(sidebar, /useSidebarModel\(\)/)
  assert.match(items, /from '\.\/board-store'/)
  assert.match(items, /const query = useSessionListQuery\(\)/)
  assert.doesNotMatch(items, connection)
})

test('escape never leaves the session page; returning to the board needs an explicit action', async () => {
  const sources = Object.fromEntries(
    await Promise.all(
      [
        'src/app/SessionShell.tsx',
        // 第 4 期主页改为会话：手机顶栏左侧是看板入口（BoardEntry）；v2 的侧栏外壳与会话分组、
        // 主页草稿与手机会话页的部件同样不得用 Esc 离开。
        'src/app/BoardEntry.tsx',
        'src/app/SessionHome.tsx',
        'src/features/board/NewTaskDraft.tsx',
        'src/features/board/PhoneAgentStrip.tsx',
        'src/features/chat/SessionPhoneHeader.tsx',
        'src/app/ShellSidebar.tsx',
        'src/features/board/SidebarSessions.tsx',
        'src/features/chat/SessionWorkspace.tsx',
        'src/features/chat/SessionInspector.tsx',
        'src/features/chat/SessionPageHeader.tsx',
        'src/features/chat/InspectorChanges.tsx',
        'src/features/chat/FocusSession.tsx',
      ].map(async (file) => [file, await read(file)]),
    ),
  )
  // 会话页外壳与看板入口不监听键盘：去全部代理只认侧栏入口、⌘1（应用级快捷键）与
  // 浏览器 / 原生返回。
  for (const file of [
    'src/app/SessionShell.tsx',
    'src/app/BoardEntry.tsx',
    'src/app/SessionHome.tsx',
  ])
    assert.doesNotMatch(sources[file], /keydown|Escape|onKeyDown/, file)
  // 任何 Esc 处理附近都不出现路由跳转或返回动作。
  for (const [file, source] of Object.entries(sources)) {
    for (const match of source.matchAll(/'Escape'/g)) {
      const handler = source.slice(Math.max(0, match.index - 200), match.index + 400)
      assert.doesNotMatch(
        handler,
        /navigate\(|history\.back|backToBoard|onBack\b|onOpenSession|requestBack|routeBack/,
        `${file} binds Escape near a navigation call`,
      )
    }
  }
  // 检查器覆盖层只登记为返回层（Esc / 原生返回先关它），不登记路由返回。
  assert.match(
    sources['src/features/chat/SessionWorkspace.tsx'],
    /useBackLayer\(active && overlayMode && overlayOpen, 100, close\)/,
  )
  // 规格 v2：⌘1 是全部代理（openBoard）的默认键，⇧⌘B 归会话树（showBoard 与 ⌘[ 的返回看板都已退役）；
  // Esc 不能被设成离开会话页或打开别处的动作（校验拒绝，归一化退回默认值）。
  const shortcuts = await import('../../shared/shortcuts.mjs')
  for (const retired of ['backToBoard', 'showBoard'])
    assert.equal(shortcuts.SHORTCUT_ACTIONS.includes(retired), false)
  assert.equal(shortcuts.DEFAULT_SHORTCUTS.sessionTree, 'Mod+Shift+KeyB')
  assert.equal(shortcuts.DEFAULT_SHORTCUTS.openBoard, 'Mod+Digit1')
  for (const action of ['sessionTree', 'openBoard', 'openSession', 'sideChat']) {
    for (const binding of ['Escape', 'Mod+Escape'])
      assert.throws(
        () =>
          shortcuts.validateShortcutBindings({
            ...shortcuts.DEFAULT_SHORTCUTS,
            [action]: binding,
          }),
        (error) => error instanceof shortcuts.ShortcutValidationError,
        `${action} ${binding}`,
      )
    assert.equal(
      shortcuts.normalizeShortcutBindings({ [action]: 'Escape' })[action],
      shortcuts.DEFAULT_SHORTCUTS[action],
    )
  }
  // 键盘 Esc 走 BackNavigation 的返回层，路由层返回只挂在手机原生返回上（行为见 back-navigation.test.mjs）。
  const app = await read('src/App.tsx')
  assert.match(app, /<BackNavigation onRouteBack=\{mobileLayout \? routeBack : undefined\} \/>/)
})

test('external open requests and the end-of-run reveal land on inspector tabs', async () => {
  const workspace = await read('src/features/chat/SessionWorkspace.tsx')
  // 旧入口（网页预览、任务内容事件、自动展示）仍写会话上下文页 store；这里映射到检查器标签与子视图。
  for (const [kind, target] of [
    ['changes', "{ tab: 'changes', changesView: 'workspace' }"],
    ['files', "{ tab: 'changes', changesView: 'session' }"],
    ['sources', "{ tab: 'files' }"],
    ['browser', "{ tab: 'preview' }"],
    ['plan', "{ tab: 'plan' }"],
    ['side-chat', "{ tab: 'side-chat' }"],
  ])
    assert.ok(
      new RegExp(`case '${kind}':\\s*return ${target.replace(/[{}]/g, '\\$&')}`).test(workspace),
      `${kind} -> ${target}`,
    )
  assert.match(
    workspace,
    /case 'terminal':\s*case 'processes':\s*return \{ tab: 'terminal', terminalView: kind \}/,
  )
  assert.match(workspace, /addEventListener\(TASK_CONTENT_REQUESTED, request\)/)
  // 运行结束且留下改动：宽屏自动展开“改动 · 本会话”；覆盖层不自动弹出，避免盖住刚出来的回复。
  assert.match(
    workspace,
    /useSessionContextAutoReveal\(\{[\s\S]*?enabled: !overlayMode,[\s\S]*?onReveal: \(\) => reveal\('files'\),?\s*\}\)/,
  )
})

test('the inspector sits after the conversation with a keyboard separator only when docked', async () => {
  // 取代旧上下文面板的布局守卫（session-context-regions 仍覆盖手机外壳的 SessionContextLayout）：
  // 对话列在前、检查器在后；并排时有可聚焦、可用方向键调宽的分隔条，覆盖层没有分隔条也不抢对话列的位置。
  const workspace = await read('src/features/chat/SessionWorkspace.tsx')
  const conversation = workspace.indexOf('data-session-conversation')
  const aside = workspace.indexOf('data-inspector-aside')
  const overlay = workspace.indexOf('data-inspector-overlay')
  assert.ok(conversation > 0 && conversation < aside && aside < overlay)
  const docked = workspace.slice(workspace.indexOf('{!overlayMode && open && ('), overlay)
  assert.match(docked, /role="separator"\s+tabIndex=\{0\}\s+aria-orientation="vertical"/)
  assert.match(docked, /aria-controls=\{panelId\}/)
  assert.match(docked, /\['ArrowLeft', 'ArrowRight', 'Enter'\]\.includes\(event\.key\)/)
  const floating = workspace.slice(
    overlay,
    workspace.indexOf('</SessionInspectorContext.Provider>'),
  )
  assert.doesNotMatch(floating, /role="separator"/)
  // 覆盖层贴右、从会话页头下方开始（页头的“⋯”、侧聊与开关始终可点），没有遮罩。
  assert.match(floating, /absolute right-0 bottom-0/)
  assert.match(floating, /top: headerHeight/)
  // 宽度够时对话内容让出覆盖层的宽度（停止键、发送键不被盖住）；不够时保持纯覆盖。
  assert.match(
    workspace,
    /width - SESSION_INSPECTOR_OVERLAY_WIDTH >= SESSION_CONVERSATION_MIN_WIDTH\s*\?\s*SESSION_INSPECTOR_OVERLAY_WIDTH\s*:\s*0/,
  )
  assert.match(workspace, /style=\{overlayInset \? \{ paddingRight: overlayInset \} : undefined\}/)
  // 收起时对话列独占：检查器只在 open 时渲染。例外只有手机底部表单的退场动画期间（sheetLeaving 只在手机表单时
  // 成立，退场结束即卸载）；右侧覆盖层分支走到时一定是打开的。
  assert.match(workspace, /const sheetLeaving = sheetMode && sheetPresence\.leaving/)
  assert.match(workspace, /\{overlayMode && \(open \|\| sheetLeaving\) && \(/)
})
