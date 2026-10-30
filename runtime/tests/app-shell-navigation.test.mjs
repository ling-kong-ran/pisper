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
  SHELL_TABS,
  SHELL_TAB_PAGES,
  activeShellTab,
  shellBackPath,
} from '../../src/app/shell-tabs.ts'
// “更多”启动台的分组、能力裁剪与平台禁用在 more-navigation.test.mjs。
import { MORE_PAGES } from '../../src/app/more-navigation.ts'
import { getPageMeta } from '../../src/app/navigation.ts'
import { getNavigation } from '../../src/app/palette-navigation.ts'
import { ACTIVITY_NAVIGATION_STATE, settingsOwnsPage } from '../../src/app/settings-navigation.ts'
import { LEGACY_RUNTIME_CAPABILITIES } from '../../src/types/runtime-capabilities.ts'
import { parseFleetSystemInfo } from '../../src/features/board/useFleetSystemInfo.ts'
import { formatCompactDuration } from '../../src/lib/format.ts'
import { AppTitlebar } from '../../src/app/AppTitlebar.tsx'

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

test('each page lights exactly one primary tab, honouring the entry origin of dual-owned pages', () => {
  // 第 4 期：会话 / 看板 / 组件 / 更多 / 设置；“会话”标签另走主页逻辑，不在 SHELL_TAB_PAGES 里。
  assert.deepEqual([...SHELL_TABS], ['session', 'board', 'components', 'more', 'settings'])
  assert.equal(SHELL_TAB_PAGES.settings, 'config')
  assert.equal('session' in SHELL_TAB_PAGES, false)
  const tab = (page, state) => activeShellTab(page, settingsOwnsPage(page, state))
  assert.equal(tab('board'), 'board')
  assert.equal(tab('session'), 'session')
  assert.equal(tab('chat'), 'session')
  assert.equal(tab('components'), 'components')
  assert.equal(tab('more'), 'more')
  assert.equal(tab('assets'), 'more')
  assert.equal(tab('workflowCreate'), 'more')
  assert.equal(tab('customComponentTool'), 'more')
  assert.equal(tab('config'), 'settings')
  // 插件、星忆等页：从设置进入点亮设置，从“更多”进入点亮更多。
  assert.equal(tab('plugins'), 'settings')
  assert.equal(tab('plugins', ACTIVITY_NAVIGATION_STATE), 'more')
  assert.equal(tab('memory', ACTIVITY_NAVIGATION_STATE), 'more')
  for (const page of ['chatHistory', 'assets', 'memory', 'workflows', 'schedules', 'channels'])
    assert.ok(MORE_PAGES.has(page), page)
})

test('component detail routes stay on the components tab and native back walks the shell hierarchy', () => {
  // 组件列表与详情：/components/:componentId 仍归“组件”标签。
  assert.equal(pageFromPath('/components/game-assets'), 'components')
  assert.equal(pageFromPath('/components/game-assets/'), 'components')
  assert.equal(pageFromPath('/components/a/b'), null)
  assert.equal(componentPath('a b'), '/components/a%20b')
  assert.equal(componentPath(''), '/components')
  // 规格 7.1：弹层关掉之后，原生返回按层级走——更多的子页回更多，组件详情回列表。
  // 第 4 期主页改为会话：看板与其余一级标签页回主页；会话是根或走历史回到来处（细则见 session-home.test）。
  const back = (path, state, canGoBack = false) => {
    const page = pageFromPath(path)
    return shellBackPath(page, path, settingsOwnsPage(page, state), {
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
  assert.equal(back('/plugins', ACTIVITY_NAVIGATION_STATE), '/more')
  // 从设置进入的插件页、设置各分区都是“设置”标签的同级页，回主页。
  assert.equal(back('/plugins'), HOME_PATH)
  assert.equal(back('/config/interface'), HOME_PATH)
  assert.equal(back('/components'), HOME_PATH)
  assert.equal(back('/components/game-assets'), '/components')
})

test('palette navigation and page meta cover the new shell pages', () => {
  const [[, shell], [, features]] = getNavigation(t, capabilities({ mcp: false }))
  // “会话”用兼容页面 id 'chat'：navigate('chat') 没有待打开的会话时落到主页，不建会话。
  assert.deepEqual(
    shell.map(([page]) => page),
    ['chat', 'board', 'components', 'more', 'config'],
  )
  assert.equal(
    features.some(([page]) => page === 'mcp'),
    false,
  )
  const meta = getPageMeta(t)
  for (const page of ['board', 'more', 'session']) assert.ok(meta[page][0], page)
})

test('shell shortcuts have defaults and older saved maps gain them without losing customisations', () => {
  assert.equal(DEFAULT_SHORTCUTS.openBoard, 'Mod+Digit1')
  assert.equal(DEFAULT_SHORTCUTS.openComponents, 'Mod+Digit2')
  assert.equal(DEFAULT_SHORTCUTS.openMore, 'Mod+Digit3')
  assert.equal(DEFAULT_SHORTCUTS.jumpToAttention, 'Alt+KeyA')
  assert.equal(DEFAULT_SHORTCUTS.backToBoard, 'Mod+BracketLeft')
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
  assert.equal(migrated.openMore, 'Mod+Digit3')
  // 未知动作被丢弃，不会让整张表回到默认。
  assert.equal(normalizeShortcutBindings({ ...legacy, retired: 'F3' }).commandPalette, 'F2')
  assert.equal(
    matchesShortcut({ code: 'KeyA', altKey: true }, DEFAULT_SHORTCUTS.jumpToAttention),
    true,
  )
  assert.equal(
    matchesShortcut({ code: 'BracketLeft', metaKey: true }, DEFAULT_SHORTCUTS.backToBoard, true),
    true,
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
    assert.equal(requestBoardNewTask('titlebar'), true)
    window.removeEventListener(BOARD_NEW_TASK_EVENT, claim)
    assert.deepEqual(sources, ['titlebar'])
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

function renderTitlebar(activeTab) {
  return renderToStaticMarkup(
    h(AppTitlebar, {
      activeTab,
      onSelectTab() {},
      onOpenPalette() {},
      onNewTask() {},
      fleet: h('span', { 'data-fleet-slot': '' }),
    }),
  )
}

test('the desktop title bar is the only primary navigation and reserves the window-control corner', async () => {
  const html = renderTitlebar('more')
  const root = html.slice(0, html.indexOf('>') + 1)
  // 整栏是拖拽区；Windows 自绘标题栏时右侧 144px 让给窗口按钮，其余平台只留 12px。
  assert.match(root, /^<header\b/)
  assert.match(root, /data-window-drag-region="true"/)
  assert.match(root, /data-app-titlebar="true"/)
  assert.match(root, /\bh-12\b/)
  assert.match(root, /\bpr-3\b/)
  assert.match(root, /\[\[data-custom-titlebar\]_&amp;\]:pr-\[144px\]/)
  // 五个主标签（会话 / 看板 / 组件 / 更多 / 设置）按顺序出现在一个带名称的导航里，当前标签用 aria-current 表达。
  const tabs = [...html.matchAll(/data-shell-tab="(\w+)"/g)].map((match) => match[1])
  assert.deepEqual(tabs, [...SHELL_TABS])
  assert.match(html, /<nav aria-label="[^"]+"/)
  assert.deepEqual(
    [...html.matchAll(/<button[^>]*aria-current="page"[^>]*data-shell-tab="(\w+)"/g)].map(
      (match) => match[1],
    ),
    ['more'],
  )
  assert.doesNotMatch(renderTitlebar(null), /aria-current/)
  // 舰队计量经插槽放入；“新任务”是默认主按钮（石墨强调色，不用琥珀）。
  assert.match(html, /data-fleet-slot=""/)
  assert.match(html, /data-variant="default"[^>]*>[\s\S]*?lucide-plus/)
  assert.doesNotMatch(html, /st-attn/)

  const [app, controls] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/layout/DesktopWindowControls.tsx', 'utf8'),
  ])
  // 外壳根节点按桥接能力标记自绘标题栏，窗口按钮只在这时挂载。
  assert.match(
    app,
    /data-custom-titlebar=\{window\.pisperDesktop\?\.customTitlebar \|\| undefined\}/,
  )
  assert.match(
    app,
    /\{window\.pisperDesktop\?\.customTitlebar && \(\s*<Suspense fallback=\{null\}>\s*<DesktopWindowControls \/>/,
  )
  // 窄化的桌面窗口永远不进手机外壳，标题栏始终保留（规格 2.7）；手机 App 平板也走桌面布局（规格 7.6）。
  assert.match(app, /const phoneShell = mobileLayout && !window\.pisperDesktop && !tabletApp/)
  // 三个 44px 窗口按钮叠在标题栏右上角：总宽 132px 落在 144px 预留区内，高度与标题栏同为 48px。
  assert.match(
    controls,
    /className="desktop-window-controls absolute right-0 top-0 z-40 flex h-12"/,
  )
  assert.equal(controls.match(/'minimize', 'maximize', 'close'/g)?.length, 1)
  // Tailwind 间距单位 4px：按钮宽度取自源码，三个按钮之和不得超出预留区。
  const buttonWidth = Number(controls.match(/grid w-(\d+) place-items-center/)?.[1]) * 4
  assert.ok(buttonWidth >= 44 && 3 * buttonWidth <= 144, `window buttons are ${buttonWidth}px`)
})
