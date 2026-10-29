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
  const [app, more, board, lane] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/app/MorePage.tsx', 'utf8'),
    readFile('src/features/board/BoardPage.tsx', 'utf8'),
    readFile('src/features/board/LaneColumn.tsx', 'utf8'),
  ])
  // 看板与更多页不走 PageFrame：外壳给它们无留白、不滚动的容器，滚动由页面自己承担；
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
  assert.match(more, /<div className="h-full min-h-0 w-full overflow-y-auto bg-background">/)
  assert.match(
    board,
    /data-board-root\s+className="relative flex h-full min-h-0 w-full min-w-0 flex-col/,
  )
  // 看板各泳道独立滚动，页面本身不出现整页滚动条。
  assert.match(
    lane,
    /data-lane-scroller=\{lane\}\s+className="relative min-h-0 flex-1 overflow-y-auto/,
  )
})
