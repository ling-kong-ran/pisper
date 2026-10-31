import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
test('migrated route pages give scroll and outer spacing ownership to a page template', async () => {
  const paths = [
    'config/ConfigPage',
    'schedules/SchedulesPage',
    'mcp/McpPage',
    'skills/SkillsPage',
    'channels/ChannelsPage',
    'memory/MemoryPage',
    'assets/AssetsPage',
    'plugins/PluginsPage',
    'workflows/WorkflowsPage',
    'custom-ui/CustomUiPage',
    'custom-ui/CustomUiToolPage',
  ]
  for (const path of paths) {
    const source = await readFile(`src/features/${path}.tsx`, 'utf8')
    assert.match(source, /<PageFrame\b/, path)
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
