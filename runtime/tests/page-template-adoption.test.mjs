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
