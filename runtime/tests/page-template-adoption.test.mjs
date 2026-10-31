import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
// A6 功能页模板（src/components/app/FeaturePage.tsx）：页面 id → 路由页面源码。迁移一页就在这里与
// src/app/more-pages.ts 的 FEATURE_TEMPLATE_PAGES 各加一行，两边必须一致。
const FEATURE_TEMPLATE_SOURCES = {
  plugins: 'plugins/PluginsPage',
  skills: 'skills/SkillsPage',
  mcp: 'mcp/McpPage',
  chatHistory: 'chat/ChatHistoryPage',
  assets: 'assets/AssetsPage',
  memory: 'memory/MemoryPage',
  workflows: 'workflows/WorkflowsPage',
  schedules: 'schedules/SchedulesPage',
  channels: 'channels/ChannelsPage',
}

// 同一文件里还有别的页面时只核对列表页那一段：工作流文件里的节点编辑器是全宽例外（PageFrame workbench）。
function templateSource(path, source) {
  if (path !== 'workflows/WorkflowsPage') return source
  return source.slice(
    source.indexOf('export function WorkflowsPage('),
    source.indexOf('function WorkflowEditorHeader('),
  )
}

test('migrated route pages give scroll and outer spacing ownership to a page template', async () => {
  // 还没迁到功能页模板的页面（组件工作室与“我的组件”整页，A7）：PageFrame 承担滚动与外边距。
  // 设置分区（ConfigPage）在 A6 第 4 批改用功能页模板，见下方“设置分区”一条。
  const paths = ['custom-ui/CustomUiPage', 'custom-ui/CustomUiToolPage']
  for (const path of paths) {
    const source = await readFile(`src/features/${path}.tsx`, 'utf8')
    assert.match(source, /<PageFrame\b/, path)
  }
  // 已迁到功能页模板的页面：FeaturePage 承担滚动区、居中 760 栏、页标题与工具行；不再叠 PageFrame
  // 或列表 / 详情两栏（行内展开取代右侧详情），也不再向壳层注册页头主操作（主操作在自己的工具行里）。
  for (const path of Object.values(FEATURE_TEMPLATE_SOURCES)) {
    const file = await readFile(`src/features/${path}.tsx`, 'utf8')
    assert.doesNotMatch(file, /usePagePrimaryAction|registerPrimaryAction/, path)
    const source = templateSource(path, file)
    assert.match(source, /<FeaturePage\b/, path)
    assert.match(source, /<FeatureToolbar\b/, path)
    assert.doesNotMatch(source, /<PageFrame\b|<ListDetailLayout\b|usePagePrimaryAction/, path)
    // 搜索框用外壳“/ 聚焦搜索”认识的 id；返回“‹ 功能”来自路由上下文（应用壳判断归属）。
    assert.match(source, /id=\{FEATURE_SEARCH_INPUT_ID\}/, path)
    assert.match(source, /back=\{\s*onBack && \{/, path)
  }
  const host = await readFile('src/features/custom-ui/CustomUiToolPage.tsx', 'utf8')
  assert.doesNotMatch(
    host,
    /calc\(100d?vh-/,
    'embedded tools must size from their actual container',
  )
})

test('full-bleed shell pages own their scroll instead of the padded page container', async () => {
  const [app, more, board] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/app/MorePage.tsx', 'utf8'),
    readFile('src/features/board/BoardPage.tsx', 'utf8'),
  ])
  // 看板与功能页不走 PageFrame：外壳给它们无留白、不滚动的容器，滚动由页面自己承担；
  // 会话页沿用聊天页的弹性容器，下方终端展开时对话区随之让高。
  const content = app.slice(app.indexOf('className={`page-content'), app.indexOf('key={page}'))
  assert.match(
    content,
    /page === 'session' \|\| page === 'chat' \? 'page-chat flex overflow-hidden p-0'/,
  )
  const fullBleed = content.match(/\[([^\]]+)\]\.includes\(page\)/)?.[1] || ''
  for (const page of ['board', 'more', 'components'])
    assert.match(fullBleed, new RegExp(`'${page}'`), page)
  assert.match(content, /`page-\$\{page\} overflow-hidden p-0`/)
  // 功能索引（A3，规格 2.7）：与全部代理同一结构——顶栏 52 固定，内容在自己的滚动区里滚动（居中 760 栏）。
  assert.match(
    more,
    /data-features-index className="flex h-full min-h-0 w-full flex-col bg-background"/,
  )
  assert.match(more, /data-features-scroller\s+className="min-h-0 flex-1 overflow-y-auto/)
  assert.match(
    more,
    /data-features-column\s+className="mx-auto flex w-full max-w-\[808px\][^"]*px-6/,
  )
  assert.match(
    board,
    /data-board-root\s+data-overview\s+className="relative flex h-full min-h-0 w-full min-w-0 flex-col/,
  )
  // 全部代理（A3 取代泳道）：顶栏 52 固定，分组列表在自己的滚动区里滚动，外壳容器不出现整页滚动条。
  assert.match(board, /data-overview-scroller\s+className="min-h-0 flex-1 overflow-y-auto/)
})

test('feature-template pages are exactly the pages that render the template and skip the shared header', async () => {
  const [app, morePages, routes] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/app/more-pages.ts', 'utf8'),
    readFile('src/app/route-elements.tsx', 'utf8'),
  ])
  const declared = morePages
    .match(/FEATURE_TEMPLATE_PAGES: ReadonlySet<string> = new Set\(\[([^\]]*)\]\)/)?.[1]
    .match(/'([^']+)'/g)
    ?.map((entry) => entry.slice(1, -1))
  assert.deepEqual(declared?.sort(), Object.keys(FEATURE_TEMPLATE_SOURCES).sort())
  // 共享页头（面包屑、搜索、主操作）不在这些页上渲染，“/”落到页面工具行的搜索框。
  assert.match(app, /!FEATURE_TEMPLATE_PAGES\.has\(page\)/)
  assert.match(
    app,
    /else if \(FEATURE_TEMPLATE_PAGES\.has\(page\)\)\s*document\.getElementById\(FEATURE_SEARCH_INPUT_ID\)\?\.focus\(\)/,
  )
  assert.match(app, /onBackToFeatures: moreOwned \? backToMore : undefined/)
  // 外壳给它们无留白、不滚动的容器（page-content overflow-hidden p-0），滚动由模板自己的滚动区承担。
  const content = app.slice(app.indexOf('className={`page-content'), app.indexOf('key={page}'))
  const fullBleed = content.match(/\[([^\]]+)\]\.includes\(page\)/)?.[1] || ''
  for (const [page, path] of Object.entries(FEATURE_TEMPLATE_SOURCES)) {
    assert.match(fullBleed, new RegExp(`'${page}'`), page)
    // 路由把“‹ 功能”的返回动作交给页面，页面自己的搜索替代了壳层的共享搜索词。
    const component = path.split('/')[1]
    const route = routes.slice(
      routes.indexOf(`<${component}`),
      routes.indexOf('/>', routes.indexOf(`<${component}`)),
    )
    assert.match(route, /onBack=\{context\.onBackToFeatures\}/, page)
    assert.doesNotMatch(route, /query=\{context\.query\}|registerPrimaryAction/, page)
  }
})

test('the workflow node editor stays full width with one 52px header of its own', async () => {
  const [page, app, routes] = await Promise.all([
    readFile('src/features/workflows/WorkflowsPage.tsx', 'utf8'),
    readFile('src/App.tsx', 'utf8'),
    readFile('src/app/route-elements.tsx', 'utf8'),
  ])
  // 规格 2.7“节点编辑器全宽例外”：画布 + 属性栏铺满主区（PageFrame workbench），只统一页头与返回——
  // 共享页头不再叠在编辑器上，编辑器自己的页头是 52px 拖拽区（控件不拖），Windows 自绘标题栏让出 144px。
  const editor = page.slice(page.indexOf('function WorkflowEditorHeader('))
  assert.match(editor, /<PageFrame variant="workbench">/)
  assert.match(
    editor,
    /data-workflow-editor-header\s+data-focus-region="header"\s+data-window-drag-region\s+className="[^"]*min-h-\[52px\][^"]*\[\[data-custom-titlebar\]_&\]:pr-\[144px\]/,
  )
  // 返回回到列表（读屏名“返回工作流”），h1 是新建 / 编辑标题，手机上只给读屏（页名在外壳顶栏）。
  assert.match(editor, /aria-label=\{t\('workflows:workflowsPage\.backToWorkflows'\)\}/)
  assert.match(editor, /<h1\s+className=\{cn\([\s\S]*?phone && 'sr-only'/)
  // 保存草稿 / 试运行 / 发布在编辑器页头里直接调用，读取中与失败时页头（返回与标题）也在。
  assert.match(editor, /saveWorkflow\('draft'\)/)
  assert.match(editor, /void publishWorkflow\(\)/)
  assert.match(editor, /running \? void stopWorkflow\(\) : openRunDialog\(\)/)
  assert.match(editor, /<WorkflowEditorHeader title=\{title\} onBack=\{backToList\} t=\{t\} \/>/)
  assert.match(
    app,
    /\['board', 'session', 'more', 'chat', 'workflowCreate', 'config'\]\.includes\(page\)/,
  )
  const builder = routes.slice(routes.indexOf('<WorkflowBuilder'))
  assert.doesNotMatch(builder.slice(0, builder.indexOf('/>')), /registerPrimaryAction/)
})

test('device pages under /config open as feature-template pages with centralized platform support', async () => {
  const [config, device, app, routes, capabilities, more, remote, pet, server, workspace] =
    await Promise.all([
      readFile('src/features/config/ConfigPage.tsx', 'utf8'),
      readFile('src/features/config/DeviceFeaturePage.tsx', 'utf8'),
      readFile('src/App.tsx', 'utf8'),
      readFile('src/app/route-elements.tsx', 'utf8'),
      readFile('src/types/runtime-capabilities.ts', 'utf8'),
      readFile('src/app/more-navigation.ts', 'utf8'),
      readFile('src/features/config/RemoteAccessSettings.tsx', 'utf8'),
      readFile('src/features/config/DesktopPetSettings.tsx', 'utf8'),
      readFile('src/features/config/MobileServerSettings.tsx', 'utf8'),
      readFile('src/features/config/RemoteWorkspaceSettings.tsx', 'utf8'),
    ])
  // /config/remote-access、mobile-server、desktop-pet（规格 2.4）交给 DeviceFeaturePage：功能页模板，不套设置的 PageFrame。
  assert.match(
    config,
    /if \(section === 'remote-access' \|\| section === 'mobile-server' \|\| section === 'desktop-pet'\)\s*return \(\s*<DeviceFeaturePage/,
  )
  assert.match(device, /<FeaturePage\b/)
  assert.match(device, /back=\{\s*onBack && \{/)
  assert.doesNotMatch(device, /<PageFrame\b/)
  // 设置搜索跳到这三个分区仍按分区锚点定位。
  assert.match(device, /data-config-card=\{CONFIG_SECTION_ANCHOR\}/)
  // 共享页头不在 /config 下渲染（设备页与设置分区都用模板）；路由把“‹ 功能”的返回动作交给 ConfigPage。
  assert.match(app, /'workflowCreate', 'config'\]\.includes\(page\)/)
  assert.match(routes, /onBackToFeatures=\{context\.onBackToFeatures\}/)
  // 平台判断集中在 deviceFeatureSupport：功能索引的卡片与页面读同一份结论；页面不支持时显示禁用开关并写原因。
  assert.match(capabilities, /export function deviceFeatureSupport\(/)
  assert.match(more, /deviceFeatureSupport\(capabilities, section, \{ mobileApp \}\)/)
  assert.match(device, /deviceFeatureSupport\(capabilities, section, \{ mobileApp \}\)/)
  assert.match(device, /<Switch checked=\{false\} disabled aria-label=\{name\} \/>/)
  // 内容是功能页的分区与列表行，不再是设置卡片墙。
  for (const [name, source] of Object.entries({ remote, pet, server, workspace })) {
    assert.match(source, /<FeatureSection\b/, name)
    assert.doesNotMatch(source, /SettingsCard\b|SettingsCardHeader|<Panel\b/, name)
  }
  // 琥珀只给需要你的事：启动失败、防火墙待授权、待审批连接；连上的手机服务器不再是绿色条。
  assert.match(remote, /attention=\{listenState === 'failed'\}/)
  assert.doesNotMatch(server, /bg-success-soft|text-success-strong/)
})

// A6 第 4 批（规格 2.6、6.4，效果图 desktop-settings-light.png）：设置分区与功能页同一个模板——52 拖拽区、
// 自己的滚动区、居中 760 栏；页标题是目录里的短名（模型 / 界面…）加一句说明，主区不再重复“设置 / 模型配置”
// 面包屑与第二个大标题。设置搜索在桌面设置侧栏、手机“‹ 设置”下面；“快速设置”在页内工具行，
// 仍经应用壳的主操作注册表触发（新手引导可以在模型页挂载前排队）。取代原“ConfigPage 必须用 PageFrame”。
test('settings sections render in the feature-page template without a duplicate breadcrumb or title', async () => {
  const [config, app, routes, navigation, header, primitives] = await Promise.all([
    readFile('src/features/config/ConfigPage.tsx', 'utf8'),
    readFile('src/App.tsx', 'utf8'),
    readFile('src/app/route-elements.tsx', 'utf8'),
    readFile('src/app/settings-navigation.ts', 'utf8'),
    readFile('src/components/layout/PageHeader.tsx', 'utf8'),
    readFile('src/features/config/settings-primitives.tsx', 'utf8'),
  ])
  assert.match(
    config,
    /<FeaturePage\s+data-settings-page=\{current\}\s+title=\{settingsSectionShortLabel\(t, current\)\}/,
  )
  assert.match(config, /description=\{settingsLede\(t, current\)\}/)
  assert.doesNotMatch(config, /<PageFrame\b|settings-wide/)
  assert.match(config, /data-config-card=\{CONFIG_SECTION_ANCHOR\}/)
  // 快速设置：页内按钮 → 壳层注册表（模型分区注册），路由把触发函数交给页面。
  assert.match(config, /data-settings-quick-setup onClick=\{invokePrimaryAction\}/)
  assert.match(routes, /invokePrimaryAction=\{context\.invokePrimaryAction\}/)
  assert.match(app, /invokePrimaryAction,\n\s+pendingAsset/)
  // 短名在设置目录、页标题与手机顶栏页名三处共用一个函数。
  assert.match(navigation, /export function settingsSectionShortLabel\(/)
  assert.match(
    app,
    /settingsOwned && isSettingsSection\(configSection\)\s*\?\s*settingsSectionShortLabel\(t, configSection\)/,
  )
  // 共享页头里设置专属的面包屑、大标题、搜索槽与主操作都已移除。
  assert.doesNotMatch(
    header,
    /titleBlock|searchSlot|settingsOwned|configSection|onPrimary|quickSetup/,
  )
  // 手机设置分区：“‹ 设置”下面是设置搜索，不随内容滚走。
  assert.match(
    app,
    /<MobileSettingsBack onBack=\{\(\) => routerNavigate\(SETTINGS_LIST_PATH\)\} \/>\s*<div data-mobile-settings-search[^>]*>\s*\{configSearch\(\)\}/,
  )
  // 设置是细线分隔的一行行设置，不是带框卡片墙（规格 6.4）。
  assert.match(
    primitives,
    /export function SettingsCard\b[\s\S]*?rounded-none border-x-0 border-t border-b-0/,
  )
  assert.match(
    primitives,
    /export function SettingsGroup\b[\s\S]*?divide-y divide-stroke-soft border-y border-stroke-soft/,
  )
})
