import assert from 'node:assert/strict'
import test from 'node:test'
import { TOOL_CATALOG, createAppTools } from '../tools/registry.mjs'

function dashboard(services = []) {
  return {
    services,
    metrics: { totalServices: services.length },
  }
}

test('structured MCP tools list and mutate services without shell or local HTTP calls', async () => {
  assert.ok(TOOL_CATALOG.some((tool) => tool.id === 'mcp_list'))
  assert.ok(TOOL_CATALOG.some((tool) => tool.id === 'mcp_manage'))
  const calls = []
  let current = dashboard([])
  const mcpRuntime = {
    async list(options) {
      calls.push({ action: 'list', options })
      return current
    },
    async add(config) {
      calls.push({ action: 'add', config })
      current = dashboard([
        {
          id: 'pencil',
          name: config.name,
          transport: config.transport,
          command: config.command,
          workingDirectory: config.cwd,
          tools: [],
        },
      ])
      return current
    },
    async update(id, config) {
      calls.push({ action: 'update', id, config })
      return current
    },
    async remove(id) {
      calls.push({ action: 'delete', id })
      current = dashboard([])
      return true
    },
    async test(id) {
      calls.push({ action: 'test', id })
      return current
    },
    async setToolEnabled(id, toolName, enabled) {
      calls.push({ action: 'set_tool_enabled', id, toolName, enabled })
      return current
    },
  }
  const tools = createAppTools({ enabledTools: ['mcp_list', 'mcp_manage'], mcpRuntime })
  const list = tools.find((tool) => tool.name === 'mcp_list')
  const manage = tools.find((tool) => tool.name === 'mcp_manage')

  await list.execute('list-1', { refresh: false })
  const command = 'C:\\Users\\lkr\\.pencil\\mcp\\server.exe'
  await manage.execute(
    'manage-1',
    {
      action: 'add',
      config: {
        name: 'Pencil',
        transport: 'stdio',
        command,
        args: ['--app', 'desktop'],
        cwd: 'C:\\Users\\lkr\\.pencil',
      },
    },
    new AbortController().signal,
  )
  await manage.execute('manage-2', { action: 'delete', id: 'pencil' }, new AbortController().signal)

  assert.equal(calls[1].config.command, command)
  assert.deepEqual(
    calls.map((call) => call.action),
    ['list', 'add', 'delete', 'list'],
  )
})
