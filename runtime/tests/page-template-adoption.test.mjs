import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
// A6 功能页模板（src/components/app/FeaturePage.tsx）：页面 id → 路由页面源码。迁移一页就在这里与
// src/app/more-pages.ts 的 FEATURE_TEMPLATE_PAGES 各加一行，两边必须一致。
const FEATURE_TEMPLATE_SOURCES = {
  plugins: 'plugins/PluginsPage',
  skills: 'skills/SkillsPage',
  mcp: 'mcp/McpPage',
}

test('migrated route pages give scroll and outer spacing ownership to a page template', async () => {
  // 还没迁到功能页模板的页面：PageFrame 承担滚动与外边距。
  const paths = [
    'config/ConfigPage',
    'schedules/SchedulesPage',
    'channels/ChannelsPage',
    'memory/MemoryPage',
    'assets/AssetsPage',
    'workflows/WorkflowsPage',
    'custom-ui/CustomUiPage',
    'custom-ui/CustomUiToolPage',
  ]
  for (const path of paths) {
    const source = await readFile(`src/features/${path}.tsx`, 'utf8')
    assert.match(source, /<PageFrame\b/, path)
  }
  // 已迁到功能页模板的页面：FeaturePage 承担滚动区、居中 760 栏、页标题与工具行；不再叠 PageFrame
  // 或列表 / 详情两栏（行内展开取代右侧详情），也不再向壳层注册页头主操作（主操作在自己的工具行里）。
  for (const path of Object.values(FEATURE_TEMPLATE_SOURCES)) {
    const source = await readFile(`src/features/${path}.tsx`, 'utf8')
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
  assert.match(app, /!FEATURE_TEMPLATE_PAGES\.has\(page\) &&/)
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
