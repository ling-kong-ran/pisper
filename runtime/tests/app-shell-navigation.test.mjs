import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_ACTIONS,
  matchesShortcut,
  normalizeShortcutBindings,
} from '../../shared/shortcuts.mjs'
import {
  HOME_ENTRY_STATE,
  HOME_PATH,
  NEW_SESSION_ROUTE_ID,
  componentPath,
  isHomeEntry,
  legacyHashPath,
  pageFromPath,
  pagePath,
  sessionIdFromPath,
  sessionPath,
} from '../../src/app/routes.ts'
import {
  PHONE_SHELL_TABS,
  SHELL_TAB_PAGES,
  activeShellTab,
  requestedPathname,
  sessionTabNavigation,
  shellBackPath,
} from '../../src/app/shell-tabs.ts'
// “更多”启动台的分组、能力裁剪与平台禁用在 more-navigation.test.mjs。
import { MORE_PAGES } from '../../src/app/more-navigation.ts'
import { getPageMeta } from '../../src/app/navigation.ts'
import { getNavigation } from '../../src/app/palette-navigation.ts'
import { settingsOwnsRoute } from '../../src/app/settings-navigation.ts'
import { LEGACY_RUNTIME_CAPABILITIES } from '../../src/types/runtime-capabilities.ts'
import { parseFleetSystemInfo } from '../../src/features/board/useFleetSystemInfo.ts'
import { formatCompactDuration } from '../../src/lib/format.ts'
import { ShellSidebar, SidebarRail } from '../../src/app/ShellSidebar.tsx'

const t = (key) => key

function capabilities(features = {}) {
  return {
    ...LEGACY_RUNTIME_CAPABILITIES,
    features: { ...LEGACY_RUNTIME_CAPABILITIES.features, ...features },
  }
}

test('the session home owns "/" while the board and sessions keep their /board paths', () => {
  // 第 4 期主页改为会话（负责人决定）：'/' 归会话页，未知页面 id 回主页；看板仍在 /board，
  // 会话仍挂在 /board/:sessionId 下（旧链接、通知与 TUI 不受影响）。
  assert.equal(pagePath('board'), '/board')
  assert.equal(pagePath('more'), '/more')
  assert.equal(pagePath('session'), `/board/${NEW_SESSION_ROUTE_ID}`)
  assert.equal(pagePath('unknown'), HOME_PATH)
  assert.equal(pageFromPath(HOME_PATH), 'session')
  assert.equal(pageFromPath('/board'), 'board')
  assert.equal(pageFromPath('/board/'), 'board')
  assert.equal(pageFromPath('/board/abc-1'), 'session')
  assert.equal(pageFromPath('/board/abc/extra'), null)
  assert.equal(pageFromPath('/more'), 'more')
  assert.equal(pageFromPath('/components'), 'components')
  assert.equal(pageFromPath('/chat/history'), 'chatHistory')
  assert.equal(sessionPath('a b'), '/board/a%20b')
  assert.equal(sessionPath(), '/board/new')
  assert.equal(sessionIdFromPath('/board/a%20b'), 'a b')
  assert.equal(sessionIdFromPath('/board/%E0%A4%A'), '')
  assert.equal(sessionIdFromPath('/more'), '')
})

test('legacy hashes keep resolving and never create a session on startup', () => {
  assert.equal(legacyHashPath('#board'), '/board')
  assert.equal(legacyHashPath('#more'), '/more')
  assert.equal(legacyHashPath('#session'), null)
  assert.equal(legacyHashPath('#/board'), null)
})

test('each page lights exactly one primary entry; feature pages have a single home under 功能', () => {
  // 规格 v2：桌面标题栏标签退役，一级入口在侧栏（全部代理 / 功能 / 设置）；手机标签栏（A4 前）仍是
  // 会话 / 看板 / 更多 / 设置。“会话”另走主页逻辑，不在 SHELL_TAB_PAGES 里；组件工作室不再是一级入口。
  assert.deepEqual([...PHONE_SHELL_TABS], ['session', 'board', 'more', 'settings'])
  assert.equal('components' in SHELL_TAB_PAGES, false)
  assert.equal(SHELL_TAB_PAGES.settings, 'config')
  assert.equal('session' in SHELL_TAB_PAGES, false)
  // 双归属退役（规格 2.6）：归属只看路由，不再看历史条目里的进入来源；/config 的分区决定设置还是功能。
  const tab = (page, section = 'models') => activeShellTab(page, settingsOwnsRoute(page, section))
  assert.equal(tab('board'), 'board')
  assert.equal(tab('session'), 'session')
  assert.equal(tab('chat'), 'session')
  // 组件工作室归“功能”（规格 2.4：功能 › 扩展），侧栏点亮“功能”。
  assert.equal(tab('components'), 'more')
  assert.equal(tab('more'), 'more')
  assert.equal(tab('assets'), 'more')
  assert.equal(tab('workflowCreate'), 'more')
  assert.equal(tab('customComponentTool'), 'more')
  for (const section of ['models', 'interface', 'notifications', 'shortcuts', 'updates', 'about'])
    assert.equal(tab('config', section), 'settings', section)
  // 插件、MCP、技能、星忆、渠道只在功能里；/config 下的远程访问、手机服务器、桌面宠物同样是功能页。
  for (const page of ['plugins', 'mcp', 'skills', 'memory', 'channels'])
    assert.equal(tab(page), 'more', page)
  for (const section of ['remote-access', 'mobile-server', 'desktop-pet'])
    assert.equal(tab('config', section), 'more', section)
  for (const page of ['chatHistory', 'assets', 'memory', 'workflows', 'schedules', 'channels'])
    assert.ok(MORE_PAGES.has(page), page)
})

test('component detail routes stay on the components tab and native back walks the shell hierarchy', () => {
  // 组件列表与详情：/components/:componentId 仍是组件页（侧栏点亮“功能”）。
  assert.equal(pageFromPath('/components/game-assets'), 'components')
  assert.equal(pageFromPath('/components/game-assets/'), 'components')
  assert.equal(pageFromPath('/components/a/b'), null)
  assert.equal(componentPath('a b'), '/components/a%20b')
  assert.equal(componentPath(''), '/components')
  // 规格 7.1：弹层关掉之后，原生返回按层级走——更多的子页回更多，组件详情回列表。
  // 第 4 期主页改为会话：看板与其余一级标签页回主页；会话是根或走历史回到来处（细则见 session-home.test）。
  const back = (path, state, canGoBack = false) => {
    const page = pageFromPath(path)
    return shellBackPath(page, path, settingsOwnsRoute(page, path.split('/')[2] || 'models'), {
      homeEntry: isHomeEntry(state),
      canGoBack,
    })
  }
  assert.equal(back(HOME_PATH), null)
  assert.equal(back('/board'), HOME_PATH)
  assert.equal(back('/board/abc'), null)
  assert.equal(back('/board/abc', undefined, true), -1)
  assert.equal(back('/board/abc', HOME_ENTRY_STATE, true), null)
  assert.equal(back('/more'), HOME_PATH)
  assert.equal(back('/assets'), '/more')
  assert.equal(back('/chat/history'), '/more')
  assert.equal(back('/workflows/new'), '/more')
  assert.equal(back('/tools/components/game-assets'), '/more')
  // 功能页（含 /config 下的远程访问等）回功能；设置各分区是“设置”标签的同级页，回主页。
  assert.equal(back('/plugins'), '/more')
  assert.equal(back('/plugins', { navOrigin: 'activity' }), '/more')
  assert.equal(back('/config/remote-access'), '/more')
  assert.equal(back('/config/desktop-pet'), '/more')
  assert.equal(back('/config/interface'), HOME_PATH)
  // 组件工作室归“功能 › 扩展”（规格 2.4 / 2.7），不再是一级页：组件列表回功能。
  assert.equal(back('/components'), '/more')
  assert.equal(back('/components/game-assets'), '/components')
})

test('the session tab decides from the latest requested route, not the rendered page', () => {
  // 数据路由 router.state 的形状：navigation 带着还在加载的目标，location 是已写入历史的位置。
  const idle = (pathname, state) => ({
    location: { pathname, search: '', state },
    navigation: { state: 'idle', location: undefined },
  })
  const loading = (from, to, state) => ({
    location: { pathname: from, search: '', state },
    navigation: { state: 'loading', location: { pathname: to } },
  })
  assert.equal(requestedPathname(idle('/board')), '/board')
  assert.equal(requestedPathname(loading(HOME_PATH, '/board')), '/board')
  // 没有进行中的导航：与按已渲染的 page 判断相同——会话页与主页不再压入历史，其余回主页。
  for (const path of [HOME_PATH, '/board/abc', sessionPath()])
    assert.equal(sessionTabNavigation(idle(path)), null, path)
  for (const path of ['/board', '/more', '/components', '/config/models', '/assets'])
    assert.deepEqual(sessionTabNavigation(idle(path)), { to: HOME_PATH }, path)
  // ⇧⌘B / ⌘2 已把看板写进历史、界面还停在主页（React 的 transition 尚未提交）：路由器已在 /board，回主页。
  assert.deepEqual(sessionTabNavigation(idle('/board')), { to: HOME_PATH })
  // 离开主页 / 会话页的懒加载导航还没写入历史：原地 replace（保留条目状态）撤销它，不压重复条目。
  assert.deepEqual(sessionTabNavigation(loading(HOME_PATH, '/board')), {
    to: HOME_PATH,
    replace: true,
    state: undefined,
  })
  assert.deepEqual(sessionTabNavigation(loading('/board/abc', '/board', HOME_ENTRY_STATE)), {
    to: '/board/abc',
    replace: true,
    state: HOME_ENTRY_STATE,
  })
  // 从看板出发、正在去别的一级页：回主页；正在去主页或会话页：什么都不做（不叠第二条）。
  assert.deepEqual(sessionTabNavigation(loading('/board', '/more')), { to: HOME_PATH })
  assert.equal(sessionTabNavigation(loading('/board', HOME_PATH)), null)
  assert.equal(sessionTabNavigation(loading('/board', '/board/abc')), null)
})

test('palette navigation and page meta cover the new shell pages', () => {
  const [[, shell], [, features]] = getNavigation(t, capabilities({ mcp: false }))
  // “会话”用兼容页面 id 'chat'：navigate('chat') 没有待打开的会话时落到主页，不建会话。
  assert.deepEqual(
    shell.map(([page]) => page),
    ['chat', 'board', 'more', 'components', 'config'],
  )
  assert.equal(
    features.some(([page]) => page === 'mcp'),
    false,
  )
  const meta = getPageMeta(t)
  for (const page of ['board', 'more', 'session']) assert.ok(meta[page][0], page)
})

test('shell shortcuts have defaults and older saved maps gain them without losing customisations', () => {
  // 规格 v2 第 2.8 节：侧栏入口 ⌘1 全部代理、⌘2 功能；会话主页与组件工作室保留为默认未绑定的动作；
  // ⌘[ 的“返回看板”与 showBoard 已退役。
  assert.equal(DEFAULT_SHORTCUTS.openSession, null)
  assert.equal(DEFAULT_SHORTCUTS.openBoard, 'Mod+Digit1')
  assert.equal(DEFAULT_SHORTCUTS.openComponents, null)
  assert.equal(DEFAULT_SHORTCUTS.openMore, 'Mod+Digit2')
  assert.equal(DEFAULT_SHORTCUTS.jumpToAttention, 'Alt+KeyA')
  for (const retired of ['backToBoard', 'showBoard']) {
    assert.equal(Object.hasOwn(DEFAULT_SHORTCUTS, retired), false)
    assert.equal(SHORTCUT_ACTIONS.includes(retired), false)
  }
  assert.equal(normalizeShortcutBindings(DEFAULT_SHORTCUTS).openBoard, 'Mod+Digit1')
  const legacy = {
    commandPalette: 'F2',
    primaryAction: null,
    focusSearch: 'Slash',
    toggleSidebar: 'Mod+KeyB',
    toggleTerminal: 'Mod+Backquote',
    openSettings: 'Mod+Comma',
    focusComposer: 'Mod+Shift+KeyL',
    sendMessage: 'Enter',
    // 用户把语音输入改到了 Alt+A，新增的“跳到需要你”不能抢走它。
    voiceInput: 'Alt+KeyA',
  }
  const migrated = normalizeShortcutBindings(legacy)
  assert.deepEqual(Object.keys(migrated).sort(), [...SHORTCUT_ACTIONS].sort())
  assert.equal(migrated.commandPalette, 'F2')
  assert.equal(migrated.primaryAction, null)
  assert.equal(migrated.voiceInput, 'Alt+KeyA')
  assert.equal(migrated.jumpToAttention, null)
  // 这张表早于一级标签：入口动作直接补 v2 默认键（会话主页未绑定）。
  assert.equal(migrated.openSession, null)
  assert.equal(migrated.openBoard, 'Mod+Digit1')
  assert.equal(migrated.openMore, 'Mod+Digit2')
  // 未知动作被丢弃，不会让整张表回到默认。
  assert.equal(normalizeShortcutBindings({ ...legacy, retired: 'F3' }).commandPalette, 'F2')
  assert.equal(
    matchesShortcut({ code: 'KeyA', altKey: true }, DEFAULT_SHORTCUTS.jumpToAttention),
    true,
  )
  assert.equal(
    matchesShortcut({ code: 'Digit1', metaKey: true }, DEFAULT_SHORTCUTS.openBoard, true),
    true,
  )
  // ⌘[ 不再是任何动作的默认键。
  assert.equal(
    SHORTCUT_ACTIONS.some((action) =>
      matchesShortcut({ code: 'BracketLeft', metaKey: true }, DEFAULT_SHORTCUTS[action], true),
    ),
    false,
  )
})

test('new task requests report whether a view claimed them', async () => {
  const previous = globalThis.window
  globalThis.window = new EventTarget()
  try {
    const { BOARD_NEW_TASK_EVENT, requestBoardNewTask } =
      await import('../../src/features/board/new-task.ts')
    assert.equal(requestBoardNewTask('shortcut'), false)
    const sources = []
    const claim = (event) => {
      sources.push(event.detail.source)
      event.preventDefault()
    }
    window.addEventListener(BOARD_NEW_TASK_EVENT, claim)
    assert.equal(requestBoardNewTask('sidebar'), true)
    window.removeEventListener(BOARD_NEW_TASK_EVENT, claim)
    assert.deepEqual(sources, ['sidebar'])
  } finally {
    globalThis.window = previous
  }
})

test('fleet system info shows only values the runtime actually reported', () => {
  assert.deepEqual(parseFleetSystemInfo({ input: 10, output: 5 }, { memory: { rss: 2048 } }), {
    todayTokens: 15,
    memoryBytes: 2048,
  })
  assert.deepEqual(parseFleetSystemInfo(undefined, { memory: {} }), {
    todayTokens: null,
    memoryBytes: null,
  })
  assert.deepEqual(parseFleetSystemInfo({ day: '2026-10-29' }, { memory: { rss: -1 } }), {
    todayTokens: null,
    memoryBytes: null,
  })
})

test('fleet system info shows sub-agent slots only when the runtime reports the limit', () => {
  // 第 2 期 Runtime 的诊断接口带 subAgents（只增字段）：浮层底部显示“子代理 x/4”。
  assert.deepEqual(
    parseFleetSystemInfo(undefined, { subAgents: { running: 1, maxConcurrent: 4 } }).subAgents,
    { running: 1, max: 4 },
  )
  // 旧 Runtime 没有这个字段，或数据不完整、不合法：不显示，绝不用常量 4 冒充。
  for (const diagnostics of [
    {},
    { subAgents: { running: 1 } },
    { subAgents: { running: 1, maxConcurrent: 0 } },
    { subAgents: { running: -1, maxConcurrent: 4 } },
    { subAgents: { running: '1', maxConcurrent: 4 } },
  ])
    assert.equal('subAgents' in parseFleetSystemInfo(undefined, diagnostics), false)
})

test('compact durations stay short and never go negative', () => {
  assert.equal(formatCompactDuration(-5), '0s')
  assert.equal(formatCompactDuration(42_000), '42s')
  assert.equal(formatCompactDuration(84_000), '1m24s')
  assert.equal(formatCompactDuration(3_780_000), '1h03m')
  assert.equal(formatCompactDuration(3 * 86_400_000), '3d')
})

function renderSidebar(active, collapsed = false) {
  // 收起偏好默认展开；窄轨单独渲染，不去改 sidebar-store 的落盘状态。
  return renderToStaticMarkup(
    h(collapsed ? SidebarRail : ShellSidebar, {
      active,
      onNewTask() {},
      onSearch() {},
      onOpenBoard() {},
      onOpenFeatures() {},
      onOpenSettings() {},
      content: h('span', { 'data-groups-slot': '' }),
      railCounts: h('span', { 'data-rail-counts-slot': '' }),
    }),
  )
}

test('the sidebar is the only primary navigation and windows keep the control corner free', async () => {
  // 规格 v2 第 2.1 节：标题栏退役，侧栏宽 268（不可拖动）是唯一的一级导航；品牌行空白处是拖拽区。
  const html = renderSidebar('features')
  assert.match(html, /^<div[^>]*data-shell-sidebar="expanded"/)
  assert.match(html.slice(0, html.indexOf('>') + 1), /\bw-\[268px\]/)
  assert.match(html.slice(0, html.indexOf('>') + 1), /\bbg-sidebar\b/)
  assert.doesNotMatch(html, /cursor-col-resize|role="separator"/)
  assert.match(html, /<nav aria-label="[^"]+"/)
  assert.match(html, /<div data-window-drag-region="true" class="[^"]*\bh-\[52px\]/)
  // 四个入口 + 底部设置按顺序出现，当前入口用 aria-current 表达；分组区是插槽（会话分组懒加载）。
  const entries = [...html.matchAll(/data-sidebar-entry="(\w+)"/g)].map((match) => match[1])
  assert.deepEqual(entries, ['newTask', 'search', 'board', 'features', 'settings'])
  assert.deepEqual(
    [...html.matchAll(/<button[^>]*data-sidebar-entry="(\w+)"[^>]*aria-current="page"/g)].map(
      (match) => match[1],
    ),
    ['features'],
  )
  assert.doesNotMatch(renderSidebar(null), /aria-current/)
  assert.match(html, /data-groups-slot=""/)
  assert.doesNotMatch(html, /data-rail-counts-slot/)
  // 静态外壳不承载状态色：琥珀只在需要你的行与计数里（插槽内）。
  assert.doesNotMatch(html, /st-attn|warning-/)
  // 收起：60px 窄轨，四个入口图标 + 计数插槽 + 底部设置；展开键带 aria-expanded=false。
  const rail = renderSidebar('board', true)
  assert.match(rail, /^<nav aria-label="[^"]+"/)
  assert.match(
    await readFile('src/app/ShellSidebar.tsx', 'utf8'),
    /collapsed \? 'w-\[60px\]' : 'w-\[268px\]'/,
  )
  assert.deepEqual(
    [...rail.matchAll(/data-sidebar-entry="(\w+)"/g)].map((match) => match[1]),
    ['newTask', 'search', 'board', 'features', 'settings'],
  )
  assert.match(rail, /data-sidebar-toggle="expand" aria-expanded="false"/)
  assert.match(rail, /data-rail-counts-slot=""/)
  assert.doesNotMatch(rail, /data-groups-slot/)

  const [app, controls, ...headers] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/layout/DesktopWindowControls.tsx', 'utf8'),
    readFile('src/components/layout/PageHeader.tsx', 'utf8'),
    readFile('src/features/chat/SessionPageHeader.tsx', 'utf8'),
    readFile('src/features/board/NewTaskDraft.tsx', 'utf8'),
  ])
  const overview = await readFile('src/features/board/BoardPage.tsx', 'utf8')
  // 外壳根节点按桥接能力标记自绘标题栏，窗口按钮只在这时挂载。
  assert.match(
    app,
    /data-custom-titlebar=\{window\.pisperDesktop\?\.customTitlebar \|\| undefined\}/,
  )
  assert.match(
    app,
    /\{window\.pisperDesktop\?\.customTitlebar && \(\s*<Suspense fallback=\{null\}>\s*<DesktopWindowControls \/>/,
  )
  assert.doesNotMatch(app, /AppTitlebar|FleetMeter|data-app-titlebar/)
  // 窄化的桌面窗口永远不进手机外壳，侧栏外壳始终保留（规格 2.7）；手机 App 平板也走桌面布局（规格 7.6）。
  assert.match(app, /const phoneShell = mobileLayout && !window\.pisperDesktop && !tabletApp/)
  assert.match(app, /\{!phoneShell && \(\s*<ShellSidebar\b/)
  // 主区顶栏（各页的页头）在 Windows 自绘标题栏时右侧预留 144px，空白处是拖拽区。
  for (const header of headers) {
    assert.match(header, /\[\[data-custom-titlebar\]_&\]:pr-\[144px\]/)
    assert.match(header, /data-window-drag-region/)
  }
  // 全部代理（A3，取代泳道页的 BoardHeader）：主区顶栏 52 是一条没有任何控件的拖拽区，页头（标题、分组方式、
  // 新任务）在它下方的滚动区里，因此右上角 144px 天然空着，不需要再让位。
  assert.match(
    overview,
    /<div data-window-drag-region aria-hidden="true" className="h-\[52px\] flex-none" \/>\s*<div\s+ref=\{scrollerRef\}\s+data-overview-scroller/,
  )
  // 三个 44px 窗口按钮叠在主区右上角：总宽 132px 落在 144px 预留区内。
  assert.match(
    controls,
    /className="desktop-window-controls absolute right-0 top-0 z-40 flex h-12"/,
  )
  assert.equal(controls.match(/'minimize', 'maximize', 'close'/g)?.length, 1)
  // Tailwind 间距单位 4px：按钮宽度取自源码，三个按钮之和不得超出预留区。
  const buttonWidth = Number(controls.match(/grid w-(\d+) place-items-center/)?.[1]) * 4
  assert.ok(buttonWidth >= 44 && 3 * buttonWidth <= 144, `window buttons are ${buttonWidth}px`)
})
