import assert from 'node:assert/strict'
import test from 'node:test'
import { readFile as readFileRaw } from 'node:fs/promises'

const readFile = async (path, options) => (await readFileRaw(path, options)).replace(/\r\n?/g, '\n')

test('Pisper shell separates global navigation from task tools and keeps capability filtering', async () => {
  const [app, focus, palette, more] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/app/palette-navigation.ts', 'utf8'),
    readFile('src/app/more-navigation.ts', 'utf8'),
  ])
  assert.doesNotMatch(app, /<ChatLayoutSwitcher|<ChatLayoutNavigation|<StatusBar/)
  // 规格 v2：一级导航在 268 侧栏（手机外壳换成顶栏 ☰ + 抽屉，规格 7.1）；旧的侧栏、活动栏、工作台页头、
  // 侧栏会话列表、48px 标题栏与手机底部标签栏都已删除，不能再被挂载或导入。
  assert.match(app, /\{!phoneShell && \(\s*<ShellSidebar\b/)
  assert.match(app, /\{phoneShell && \([\s\S]{0,120}<PhoneDrawer\b/)
  // 静态与懒加载的导入都算：只把源码留在仓库里，不把它们带进外壳的模块图。
  const imports = [...app.matchAll(/^import\s[\s\S]*?\sfrom\s+'[^']+'|\bimport\('[^']+'\)/gm)].join(
    '\n',
  )
  assert.match(imports, /@\/app\/ShellSidebar/)
  for (const retired of [
    'AppTitlebar',
    'FleetMeter',
    'AppSidebar',
    'ActivityRail',
    'WorkbenchNavigation',
    'WorkbenchHeader',
    'ProjectTaskList',
    'SidebarProvider',
    'SidebarInset',
    'MobileThreePane',
    'ShellTabBar',
  ]) {
    assert.doesNotMatch(app, new RegExp(`<${retired}\\b`), `${retired} must not be mounted`)
    assert.doesNotMatch(imports, new RegExp(`\\b${retired}\\b`), `${retired} must not be imported`)
  }
  // 看板、会话页、更多页、工作流节点编辑器（全宽例外，自己的 52px 页头）、改用功能页模板的页面与
  // /config 下的全部分区（设置分区与三个设备页都用功能页模板，A6 第 3、4 批；手机设置列表自带列表）自带页头；
  // 只剩组件页沿用共享页头（A7 前）。
  assert.match(
    app,
    /const sharedPageHeader =\s*!\['board', 'session', 'more', 'chat', 'workflowCreate', 'config'\]\.includes\(page\) &&\s*!FEATURE_TEMPLATE_PAGES\.has\(page\)\n/,
  )
  assert.match(app, /\{sharedPageHeader && \(/)
  // 新任务只有一条路径：⌘N、侧栏 / 抽屉“新任务”、手机顶栏 ✎ 与命令面板都交给 startNewTask，
  // 看板认领时聚焦启动台、主页的新任务草稿认领时聚焦草稿，否则打开派发浮层。
  for (const source of ['shortcut', 'sidebar', 'topbar', 'palette'])
    assert.match(app, new RegExp(`startNewTask\\('${source}'\\)`), source)
  assert.match(
    app,
    /if \(requestedPathname\(readRouter\(\)\) !== location\.pathname\) \{\s+pendingNewTask\.current = source\s+return\s+\}\s+if \(\(page === 'board' \|\| page === 'session'\) && requestBoardNewTask\(source\)\) return\s+setDispatchOpen\(true\)/,
  )
  // 路由器已请求去别处、渲染的 page 还是离开中的那页时，请求先记下，目标页提交后再问一次（只执行一次）。
  assert.match(
    app,
    /const source = pendingNewTask\.current\s+if \(!source \|\| requestedPathname\(readRouter\(\)\) !== location\.pathname\) return\s+pendingNewTask\.current = null\s+startNewTask\(source\)/,
  )
  // 旧的“新会话”入口（历史页页头）随 A6 退役：外壳只剩派发对话框一条新建路径，不再就地发创建事件。
  assert.doesNotMatch(app, /requestSessionCreation\(''\)/)
  assert.doesNotMatch(focus, /<WorkbenchSidebarToggle/)
  // 会话页不再兼做新任务入口：没有问候语与示例卡片（见 legacy-ui-retirement）。
  assert.doesNotMatch(focus, /QuickPromptIdeas|WorkbenchGreeting/)
  // 审批模式与执行方式在输入框的盾牌芯片里（规格 v2 第 2.5 节）。
  assert.match(focus, /<ComposerModeChip/)
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
  // 设置入口与 ⌘, 在桌面落到模型分区（与旧功能栏的设置键一致）；手机外壳先到分区列表（规格 7.4）。
  assert.match(
    app,
    /\} else if \(tab === 'settings'\) \{[\s\S]{0,160}if \(phoneShell\) routerNavigate\(SETTINGS_LIST_PATH\)\s*else setConfigSection\('models'\)/,
  )
  assert.match(app, /onSettings=\{\(\) => selectShellTab\('settings'\)\}/)
})

test('Windows controls remain a narrow capability', async () => {
  const bridge = await readFile('src-tauri/src/desktop_shell/desktop_bridge.rs', 'utf8')
  // 开关位置、折叠后的不可聚焦性与键盘焦点由 smoke-frontend-workbench 的真实组件验证。
  assert.match(bridge, /window.label\(\) != "main"/)
  assert.match(bridge, /Unsupported window action/)
})
