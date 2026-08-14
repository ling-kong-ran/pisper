import assert from 'node:assert/strict'
import test from 'node:test'

import { filterToolsForExecutionMode } from '../security/execution-mode.mjs'
import { createPluginCreateTool, manifest } from '../tools/app/plugin-create.mjs'
import { TOOL_CATALOG, TOOL_PRESETS, createAppTools } from '../tools/registry.mjs'

const input = {
  id: 'example.release',
  name: 'Release tools',
  description: 'Project release helpers.',
  permissions: ['workspace-read'],
  tools: [
    {
      name: 'example_version',
      label: 'Read version',
      description: 'Read the project version.',
      scope: 'Current project package.json',
      parameters: { type: 'object', properties: {} },
    },
  ],
  entryCode: 'export async function execute() { return "1.0.0" }\n',
}

test('plugin_create is a full-access-only high-risk app tool', () => {
  assert.equal(manifest.risk, 'high')
  assert.ok(TOOL_CATALOG.some((tool) => tool.id === 'plugin_create'))
  assert.equal(TOOL_PRESETS['read-only'].includes('plugin_create'), false)
  assert.equal(TOOL_PRESETS.workspace.includes('plugin_create'), false)
  assert.equal(TOOL_PRESETS.full.includes('plugin_create'), true)
  assert.equal(createAppTools({ enabledTools: ['plugin_create'] })[0]?.name, 'plugin_create')
  assert.deepEqual(filterToolsForExecutionMode(['plugin_create'], 'workspace-write'), [])
  assert.deepEqual(filterToolsForExecutionMode(['plugin_create'], 'full-access'), ['plugin_create'])
})

test('plugin_create delegates structured input and invalidates plugin tools after installation', async () => {
  const calls = []
  const tool = createPluginCreateTool({
    executionMode: 'full-access',
    pluginRuntime: {
      async create(value) {
        calls.push({ value })
        return {
          id: value.id,
          name: value.name,
          version: '1.0.0',
          sourcePath: '/agent/plugin-sources/example.release',
          tools: ['example_version'],
          installed: { id: value.id },
        }
      },
    },
    onPluginsChanged(result) {
      calls.push({ changed: result.id })
    },
  })

  const result = await tool.execute('plugin-1', input)
  assert.deepEqual(calls, [{ value: input }, { changed: 'example.release' }])
  assert.match(result.content[0].text, /Created and installed plugin "Release tools"/)
  assert.deepEqual(result.details.tools, ['example_version'])
})

test('plugin_create refuses execution outside full-access mode', async () => {
  const tool = createPluginCreateTool({ executionMode: 'workspace-write' })
  await assert.rejects(tool.execute('plugin-2', input), /完全访问/)
})
