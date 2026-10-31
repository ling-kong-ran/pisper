import assert from 'node:assert/strict'
import { readFile, readdir } from 'node:fs/promises'
import { dirname, resolve, relative } from 'node:path'
import test from 'node:test'
import { parse } from '@babel/parser'

const root = resolve(import.meta.dirname, '../..')

async function sources(directory) {
  const entries = await readdir(directory, { withFileTypes: true })
  return (
    await Promise.all(
      entries
        .filter((entry) => entry.name !== 'vendor')
        .map(async (entry) => {
          const path = resolve(directory, entry.name)
          return entry.isDirectory() ? sources(path) : /\.(ts|tsx)$/.test(path) ? [path] : []
        }),
    )
  ).flat()
}

const paths = await sources(resolve(root, 'src'))
const files = new Map(
  await Promise.all(paths.map(async (path) => [path, await readFile(path, 'utf8')])),
)
const graph = new Map()
// 每个文件的包导入（非 @/ 与相对路径）：只记运行时导入，供“轻量入口不得到达 htm / shared 组件规范化”守卫使用。
const packageImports = new Map()
for (const [path, source] of files) {
  const dependencies = []
  const packages = []
  const visit = (node) => {
    if (!node || typeof node !== 'object') return
    if (
      [
        'ImportDeclaration',
        'ExportNamedDeclaration',
        'ExportAllDeclaration',
        'ImportExpression',
      ].includes(node.type)
    ) {
      const specifier = node.source?.value
      if (
        typeof specifier === 'string' &&
        !specifier.startsWith('@/') &&
        !specifier.startsWith('.') &&
        node.importKind !== 'type' &&
        node.exportKind !== 'type'
      )
        packages.push({ specifier, dynamic: node.type === 'ImportExpression' })
      if (
        typeof specifier === 'string' &&
        (specifier.startsWith('@/') || specifier.startsWith('.'))
      ) {
        const target = specifier.startsWith('@/')
          ? resolve(root, 'src', specifier.slice(2))
          : resolve(dirname(path), specifier)
        const resolved = [
          target,
          `${target}.ts`,
          `${target}.tsx`,
          `${target}/index.ts`,
          `${target}/index.tsx`,
        ].find((candidate) => files.has(candidate))
        if (resolved)
          dependencies.push({
            path: resolved,
            dynamic: node.type === 'ImportExpression',
            typeOnly: node.importKind === 'type' || node.exportKind === 'type',
          })
      }
    }
    for (const child of Object.values(node)) {
      if (Array.isArray(child)) child.forEach(visit)
      else if (child?.type) visit(child)
    }
  }
  visit(
    parse(source, {
      sourceType: 'module',
      plugins: ['typescript', 'jsx'],
      createImportExpressions: true,
    }),
  )
  graph.set(path, dependencies)
  packageImports.set(path, packages)
}

const name = (path) => relative(root, path).replaceAll('\\', '/')
const feature = (path) => name(path).match(/^src\/features\/([^/]+)\//)?.[1]

test('frontend runtime imports do not form dependency cycles', () => {
  const complete = new Set()
  const active = []
  const visit = (path) => {
    assert.ok(
      !active.includes(path),
      `dependency cycle: ${[...active.slice(active.indexOf(path)), path].map(name).join(' -> ')}`,
    )
    if (complete.has(path)) return
    active.push(path)
    for (const dependency of graph.get(path) || []) if (!dependency.typeOnly) visit(dependency.path)
    active.pop()
    complete.add(path)
  }
  for (const path of graph.keys()) visit(path)
})

test('cross-feature imports use explicitly declared public contracts', () => {
  for (const [path, dependencies] of graph) {
    for (const dependency of dependencies) {
      if (!feature(path) || !feature(dependency.path) || feature(path) === feature(dependency.path))
        continue
      assert.match(
        files.get(dependency.path),
        /\/\/ @public\b/,
        `${name(path)} reaches into ${name(dependency.path)}`,
      )
    }
  }
})

test('base UI, generic libraries and global stores do not depend on features', () => {
  for (const [path, dependencies] of graph) {
    if (!/^src\/(?:components\/(?:ui|app)\/|lib\/|hooks\/|stores\/)/.test(name(path))) continue
    for (const dependency of dependencies)
      assert.equal(
        feature(dependency.path),
        undefined,
        `${name(path)} depends on ${name(dependency.path)}`,
      )
  }
})

test('chat lifecycle owners cannot transitively import page composition', () => {
  const forbidden = new Set(
    ['ChatPage.tsx', 'FocusSession.tsx', 'FocusTranscript.tsx'].map((file) =>
      resolve(root, 'src/features/chat', file),
    ),
  )
  const visit = (path, seen = new Set()) => {
    if (seen.has(path)) return
    seen.add(path)
    assert.ok(!forbidden.has(path), `lifecycle depends on ${name(path)}`)
    for (const dependency of graph.get(path) || [])
      if (!dependency.typeOnly) visit(dependency.path, seen)
  }
  for (const file of ['use-chat-dock.ts', 'use-live-session-sync.ts', 'use-prompt-commands.ts'])
    visit(resolve(root, 'src/features/chat', file))
})

test('lightweight public contracts do not pull feature pages into their import graph', () => {
  const visit = (path, seen = new Set()) => {
    if (seen.has(path)) return
    seen.add(path)
    assert.ok(!/Page\.tsx$/.test(path), `public contract loads ${name(path)}`)
    for (const dependency of graph.get(path) || [])
      if (!dependency.typeOnly) visit(dependency.path, seen)
  }
  for (const file of [
    'chat/events.ts',
    'chat/web-preview-events.ts',
    'chat/chat-api.ts',
    'plugins/public.ts',
    'board/new-task.ts',
    'board/public.ts',
    'chat/public-launch.ts',
    'custom-ui/placements.ts',
  ])
    visit(resolve(root, 'src/features', file))
})

test('chat data contracts never import the board that is built on top of them', () => {
  // 看板依赖聊天的接口、事件与会话整理接口；反向导入会形成循环并把看板数据层带进聊天页。
  const visit = (path, seen = new Set()) => {
    if (seen.has(path)) return
    seen.add(path)
    assert.notEqual(feature(path), 'board', `chat contract reaches ${name(path)}`)
    for (const dependency of graph.get(path) || [])
      if (!dependency.typeOnly) visit(dependency.path, seen)
  }
  for (const file of [
    'chat-api.ts',
    'events.ts',
    'session-organization-api.ts',
    'session-workspaces.ts',
  ])
    visit(resolve(root, 'src/features/chat', file))
})

test('optional application widgets stay lazy and page header consumes feature-free slots', () => {
  const appDependencies = graph.get(resolve(root, 'src/App.tsx'))
  for (const file of [
    'src/features/desktop-pet/WebDesktopPet.tsx',
    'src/features/config/public-components.ts',
  ]) {
    const dependency = appDependencies.find((entry) => entry.path === resolve(root, file))
    assert.ok(dependency?.dynamic, `${file} must load on demand`)
  }
  for (const dependency of graph.get(resolve(root, 'src/components/layout/PageHeader.tsx'))) {
    assert.equal(feature(dependency.path), undefined)
  }
})

test('the shell keeps board data and sidebar groups out of the startup bundle', () => {
  const appDependencies = graph.get(resolve(root, 'src/App.tsx'))
  // 侧栏的会话分组、全部代理页的筛选、设置页底部状态行与窄轨计数、手机顶栏（☰ 计数）与抽屉、派发浮层
  // 依赖看板数据层（单一轮询者），设置整页的侧栏（设置目录）只在设置页需要：都按需加载。应用壳静态只认看板的两份轻量契约（新任务、侧栏命令），二者都没有导入。
  for (const file of [
    'src/features/board/sidebar.ts',
    'src/app/PhoneTopBar.tsx',
    'src/app/PhoneDrawer.tsx',
    'src/app/SettingsSidebar.tsx',
    'src/features/board/DispatchOverlay.tsx',
  ]) {
    const dependency = appDependencies.find((entry) => entry.path === resolve(root, file))
    assert.ok(dependency?.dynamic, `${file} must load on demand`)
  }
  const staticBoard = appDependencies
    .filter((entry) => !entry.dynamic && !entry.typeOnly && feature(entry.path) === 'board')
    .map((entry) => name(entry.path))
  assert.deepEqual(staticBoard, [
    'src/features/board/new-task.ts',
    'src/features/board/sidebar-commands.ts',
  ])
  assert.deepEqual(graph.get(resolve(root, 'src/features/board/sidebar-commands.ts')), [])
  // 侧栏外壳（静态进入口）只组合外壳、基础组件与侧栏偏好；会话分组、计数和更新提示经插槽传入，
  // 不直接依赖任何 Feature。
  const sidebar = resolve(root, 'src/app/ShellSidebar.tsx')
  assert.ok(appDependencies.some((entry) => entry.path === sidebar && !entry.dynamic))
  for (const dependency of graph.get(sidebar))
    assert.equal(feature(dependency.path), undefined, name(dependency.path))
  for (const file of [
    'src/app/shell-tabs.ts',
    'src/app/more-pages.ts',
    'src/app/more-navigation.ts',
  ])
    for (const dependency of graph.get(resolve(root, file)))
      assert.equal(feature(dependency.path), undefined, `${file} -> ${name(dependency.path)}`)
})

test('no route reaches the retired Dockview split view or its vendor code', () => {
  // 第 3 期退役 Dockview 的挂载，v2（A1）删除了分屏视图（ChatDockView、Dock 预览面板）：
  // 它们不能再出现在源码里；A4 卸载了 dockview-react，从入口出发（含懒加载路由）任何文件都不能再导入
  // dockview（连 import type 也不行）。
  const retired = new Set(
    ['ChatDockView.tsx', 'WebPreviewDockPanel.tsx'].map((file) =>
      resolve(root, 'src/features/chat', file),
    ),
  )
  for (const path of retired) assert.ok(!files.has(path), `${name(path)} must stay deleted`)
  const reachable = new Set()
  const visit = (path) => {
    if (reachable.has(path)) return
    reachable.add(path)
    for (const dependency of graph.get(path) || []) if (!dependency.typeOnly) visit(dependency.path)
  }
  visit(resolve(root, 'src/main.tsx'))
  // 会话路由确实在图里（懒加载边也算），否则这条守卫会因为图不完整而空转。
  for (const file of [
    'src/app/route-elements.tsx',
    'src/app/SessionShell.tsx',
    'src/features/chat/ChatPage.tsx',
    'src/features/chat/SessionWorkspace.tsx',
    'src/features/chat/SessionInspector.tsx',
    'src/features/board/SidebarSessions.tsx',
  ])
    assert.ok(reachable.has(resolve(root, file)), `${file} must be reachable from the entry`)
  for (const path of reachable) {
    assert.ok(!retired.has(path), `${name(path)} must stay unmounted`)
    const source = files.get(path)
    assert.doesNotMatch(
      source,
      /(?:^|\n)\s*import\s+[^'"]*?['"]dockview(?:-core|-react)?(?:\/[^'"]*)?['"]|import\(\s*['"]dockview/,
      `${name(path)} imports Dockview at runtime`,
    )
  }
})

test('the dormant three-pane phone shell and the retired sidebar shell never load from the entry', () => {
  // 第 4 期手机外壳换成底部标签栏 + 单屏页面（规格 7.7）：旧三栏外壳、会话上下文全屏页只停止挂载，
  // 留给 A4 删除。v2（A1）已删除旧侧栏与活动栏、旧手机标签栏、“‹ 看板”返回条、标题栏与舰队计量、
  // 会话页泳道条与 ⌘J 代理面板（规格 9.1、第 11 节）：它们不能再出现在源码里。
  // 从入口出发（含懒加载路由）的运行时导入都不能到达休眠或已删除的文件。
  const deleted = [
    'src/components/layout/MobileTabBar.tsx',
    'src/app/SessionReturnBar.tsx',
    'src/app/WorkbenchNavigation.tsx',
    'src/app/activity-navigation.ts',
    'src/app/AppTitlebar.tsx',
    'src/app/FleetMeter.tsx',
    'src/components/layout/AppSidebar.tsx',
    'src/components/layout/ActivityRail.tsx',
    'src/components/layout/StatusBar.tsx',
    'src/components/layout/WorkbenchHeader.tsx',
    'src/features/chat/ProjectTaskList.tsx',
    'src/features/board/SessionLaneStrip.tsx',
    'src/features/board/AgentPanel.tsx',
    'src/features/board/AgentMiniCard.tsx',
    'src/features/board/useFleetSummary.ts',
    // A4（规格 7.1、7.6）：手机底部标签栏、会话页“看板 N”入口与代理条退役，换成顶栏 ☰ + 抽屉。
    'src/app/ShellTabBar.tsx',
    'src/app/BoardEntry.tsx',
    'src/features/board/PhoneAgentStrip.tsx',
    'src/features/board/phone-strip.ts',
    'src/features/board/strip-layout.ts',
    'src/features/board/strip-presentation.ts',
    'src/components/app/LaneBarsIcon.tsx',
    // A4（规格 7.3）：手机看板（泳道跳转条、卡片墙、确认层）并进全部代理页的手机卡片。
    'src/features/board/PhoneBoard.tsx',
    'src/features/board/AgentCard.tsx',
    'src/features/board/LaneGlyph.tsx',
    'src/features/board/useRunningMotion.ts',
    // A4（规格 7.6）：休眠的三栏手机外壳、会话上下文全屏页、手机会话标签与 Dockview 分屏一并删除。
    'src/components/layout/MobileThreePane.tsx',
    'src/components/layout/MobileShellContext.tsx',
    'src/components/layout/mobile-shell-layout.ts',
    'src/app/mobile/MobileContextScreen.tsx',
    'src/features/chat/use-chat-dock.ts',
    'src/features/chat/dock-layout.ts',
    'src/features/chat/SessionActionsMenu.tsx',
  ].map((file) => resolve(root, file))
  for (const path of deleted) assert.ok(!files.has(path), `${name(path)} must stay deleted`)
  const dormant = new Set(deleted)
  const reachable = new Set()
  const visit = (path) => {
    if (reachable.has(path)) return
    reachable.add(path)
    for (const dependency of graph.get(path) || []) if (!dependency.typeOnly) visit(dependency.path)
  }
  visit(resolve(root, 'src/main.tsx'))
  // 新外壳确实在图里，避免图不完整时空转。
  for (const file of [
    'src/app/PhoneTopBar.tsx',
    'src/app/PhoneDrawer.tsx',
    'src/app/PhoneMenuButton.tsx',
    'src/app/ShellSidebar.tsx',
    'src/app/MorePage.tsx',
    'src/app/SessionHome.tsx',
    'src/features/board/OverviewRow.tsx',
    'src/features/chat/SessionPhoneHeader.tsx',
  ])
    assert.ok(reachable.has(resolve(root, file)), `${file} must be reachable from the entry`)
  for (const path of dormant) assert.ok(!reachable.has(path), `${name(path)} must stay unmounted`)
})

// 静态（运行时、非 import type）可达的文件集合；动态 import() 是懒加载边界，不跟。
function staticClosure(...starts) {
  const reachable = new Set()
  const visit = (path) => {
    if (reachable.has(path)) return
    reachable.add(path)
    for (const dependency of graph.get(path) || [])
      if (!dependency.dynamic && !dependency.typeOnly) visit(dependency.path)
  }
  for (const start of starts) visit(resolve(root, start))
  return reachable
}

test('native component SDK and hosts stay behind lazy boundaries (A7 S3)', () => {
  // 原生组件（规格 5.6、计划 §4.6）：htm、native/*、studio/*、hosts/* 只能从 hosts.ts / public.ts 的动态边界进入。
  // 入口闭包（main.tsx 与无条件加载的 mount-app.tsx）不得静态到达任何 custom-ui 文件或 htm。
  const entry = staticClosure('src/main.tsx', 'src/mount-app.tsx')
  assert.ok(entry.has(resolve(root, 'src/App.tsx')), 'entry closure must include the app shell')
  for (const path of entry) {
    assert.notEqual(feature(path), 'custom-ui', `entry statically loads ${name(path)}`)
    for (const entryImport of packageImports.get(path) || [])
      if (!entryImport.dynamic)
        assert.notEqual(entryImport.specifier, 'htm', `${name(path)} pulls htm into the entry`)
  }
  // 应用壳本身没有任何 custom-ui 的静态依赖（浮窗、组件页都经 import() 进入）。
  const appStatic = graph
    .get(resolve(root, 'src/App.tsx'))
    .filter((entry) => !entry.dynamic && !entry.typeOnly && feature(entry.path) === 'custom-ui')
  assert.deepEqual(
    appStatic.map((entry) => name(entry.path)),
    [],
  )

  // placements.ts 是放置宿主唯一允许静态导入的 custom-ui 文件：它的静态闭包不得到达页面、native/、studio/、
  // hosts/，也不得导入 htm 或 shared 的 manifest 规范化（宿主本体经 LazyComponentHost 懒加载）。
  const placements = staticClosure('src/features/custom-ui/placements.ts')
  assert.ok(placements.has(resolve(root, 'src/features/custom-ui/custom-ui-api.ts')))
  for (const path of placements) {
    assert.doesNotMatch(
      name(path),
      /Page\.tsx$|^src\/features\/custom-ui\/(?:native|studio|hosts)\/|^src\/features\/custom-ui\/hosts\.ts$/,
      `placements.ts loads ${name(path)}`,
    )
    for (const entryImport of packageImports.get(path) || []) {
      if (entryImport.dynamic) continue
      assert.notEqual(entryImport.specifier, 'htm', `${name(path)} imports htm`)
      assert.doesNotMatch(
        entryImport.specifier,
        /custom-ui-manifest\.mjs$/,
        `${name(path)} imports the manifest normalizer at runtime`,
      )
    }
  }
  // 宿主本体确实经 placements 的懒加载边界可达，守卫不会因为图不完整而空转。
  assert.ok(
    graph
      .get(resolve(root, 'src/features/custom-ui/placements.ts'))
      .some(
        (entry) => entry.dynamic && entry.path === resolve(root, 'src/features/custom-ui/hosts.ts'),
      ),
  )
  // htm 只出现在 native/html.ts 一处。
  for (const [path, imports] of packageImports)
    if (imports.some((entry) => entry.specifier === 'htm'))
      assert.equal(name(path), 'src/features/custom-ui/native/html.ts')

  // 看板公共契约（及组件变化订阅）不得反向到达 custom-ui：组件宿主建在看板数据之上。
  for (const file of ['board/public.ts', 'board/component-changes.ts'])
    for (const path of staticClosure(`src/features/${file}`))
      assert.notEqual(feature(path), 'custom-ui', `${file} reaches ${name(path)}`)
  assert.deepEqual(graph.get(resolve(root, 'src/features/board/component-changes.ts')), [])
})
