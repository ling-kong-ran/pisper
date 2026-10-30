import assert from 'node:assert/strict'
import test from 'node:test'
import { readFile } from 'node:fs/promises'
import {
  greetingPeriod,
  nextGreetingDelay,
  greetingFontSize,
} from '../../src/features/chat/workbench-greeting.ts'

const at = (hour, minute = 0, second = 0) => new Date(2026, 8, 25, hour, minute, second)

test('Pisper greeting changes exactly at each local time boundary', () => {
  for (const [hour, expected] of [
    [0, 5],
    [4, 5],
    [5, 0],
    [8, 0],
    [9, 1],
    [11, 1],
    [12, 2],
    [13, 2],
    [14, 3],
    [17, 3],
    [18, 4],
    [22, 4],
    [23, 5],
  ]) {
    assert.equal(greetingPeriod(at(hour)), expected, `hour ${hour}`)
  }
  assert.equal(nextGreetingDelay(at(8, 59, 59)), 1000)
  assert.equal(nextGreetingDelay(at(9)), 3 * 60 * 60 * 1000)
  assert.equal(nextGreetingDelay(at(23)), 6 * 60 * 60 * 1000)
  assert.equal(nextGreetingDelay(at(23, 59, 59)), 5 * 60 * 60 * 1000 + 1000)
})

test('Pisper greeting font measurement remains bounded and tolerates hidden containers', () => {
  assert.equal(greetingFontSize(600, 300), 30)
  assert.equal(greetingFontSize(240, 300), 24)
  assert.equal(greetingFontSize(80, 300), 20)
  for (const [width, natural] of [
    [0, 0],
    [-1, 300],
    [300, 0],
    [NaN, 300],
    [300, Infinity],
  ]) {
    assert.equal(greetingFontSize(width, natural), 30)
  }
})

test('Pisper shell separates global navigation from task tools and keeps capability filtering', async () => {
  const [app, focus, palette, more] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/app/palette-navigation.ts', 'utf8'),
    readFile('src/app/more-navigation.ts', 'utf8'),
  ])
  assert.doesNotMatch(app, /<ChatLayoutSwitcher|<ChatLayoutNavigation|<StatusBar/)
  // 一级导航在 48px 标题栏（手机外壳换成底部标签栏）；旧的侧栏、活动栏、工作台页头与
  // 侧栏会话列表不再挂载，源码保留到第 6 期统一删除。
  assert.match(app, /\{!phoneShell && \(\s*<AppTitlebar\b/)
  assert.match(app, /\{phoneShell && \([\s\S]{0,120}<ShellTabBar\b/)
  // 静态与懒加载的导入都算：只把源码留在仓库里，不把它们带进外壳的模块图。
  const imports = [...app.matchAll(/^import\s[\s\S]*?\sfrom\s+'[^']+'|\bimport\('[^']+'\)/gm)].join(
    '\n',
  )
  assert.match(imports, /@\/app\/AppTitlebar/)
  for (const retired of [
    'AppSidebar',
    'ActivityRail',
    'WorkbenchNavigation',
    'WorkbenchHeader',
    'ProjectTaskList',
    'SidebarProvider',
    'SidebarInset',
    'MobileThreePane',
  ]) {
    assert.doesNotMatch(app, new RegExp(`<${retired}\\b`), `${retired} must not be mounted`)
    assert.doesNotMatch(imports, new RegExp(`\\b${retired}\\b`), `${retired} must not be imported`)
  }
  // 看板、会话页与更多页自带页头；其余功能页沿用共享页头。
  assert.match(
    app,
    /const sharedPageHeader = !\['board', 'session', 'more', 'chat'\]\.includes\(page\)/,
  )
  assert.match(app, /\{sharedPageHeader && \(/)
  // 新任务只有一条路径：⌘N、标题栏、手机居中按钮与命令面板都交给 startNewTask，
  // 看板认领时聚焦启动台，否则打开派发浮层。
  for (const source of ['shortcut', 'titlebar', 'tabbar', 'palette'])
    assert.match(app, new RegExp(`startNewTask\\('${source}'\\)`), source)
  assert.match(
    app,
    /if \(page === 'board' && requestBoardNewTask\(source\)\) return\s+setDispatchOpen\(true\)/,
  )
  // 旧调用方在会话页里新建会话仍走就地创建事件。
  assert.match(app, /requestSessionCreation\(''\)/)
  assert.doesNotMatch(focus, /<WorkbenchSidebarToggle/)
  assert.match(focus, /<QuickPromptIdeas/)
  assert.match(focus, /<ExecutionModeSelect/)
  // 命令面板与“更多”只列出 Runtime 未裁剪的页面；壳层兜底把不可用页面送回主页
  // （第 4 期主页改为会话，与未知路径一致；主页本身不建会话）。
  assert.match(
    palette,
    /features\.filter\(\(\[page\]\) => runtimePageAvailable\(capabilities, page\)\)/,
  )
  assert.match(more, /runtimePageAvailable\(capabilities, id\)/)
  assert.match(
    app,
    /if \(capabilitiesLoaded && !runtimePageAvailable\(capabilities, page\)\) \{\s*routerNavigate\(HOME_PATH, \{ replace: true \}\)/,
  )
  // 设置标签与 ⌘, 落到模型分区，与旧功能栏的设置键一致。
  assert.match(app, /if \(tab === 'settings'\) setConfigSection\('models'\)/)
  assert.match(app, /onSettings=\{\(\) => selectShellTab\('settings'\)\}/)
})

test('Windows controls remain a narrow capability', async () => {
  const bridge = await readFile('src-tauri/src/desktop_shell/desktop_bridge.rs', 'utf8')
  // 开关位置、折叠后的不可聚焦性与键盘焦点由 smoke-frontend-workbench 的真实组件验证。
  assert.match(bridge, /window.label\(\) != "main"/)
  assert.match(bridge, /Unsupported window action/)
})
