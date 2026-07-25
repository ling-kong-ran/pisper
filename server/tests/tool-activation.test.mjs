import assert from 'node:assert/strict'
import test from 'node:test'
import { explicitlyRequestedToolNames, hotToolNames, isExplicitMemoryRememberRequest, mergePromotedToolNames, schemaOnlyToolDefinition, selectedToolNames } from '../tools/tool-activation.mjs'

const available = [
  'read', 'grep', 'find', 'ls', 'edit', 'write', 'bash',
  'web_search', 'browser_automation', 'generate_visual',
  'memory_search', 'memory_remember', 'mcp_list', 'mcp_manage',
  'get_task_list', 'update_task_list', 'discover_tools',
  'spawn_agent', 'list_agents', 'send_message', 'followup_task', 'wait_agent', 'interrupt_agent',
  'mcp_pencil_batch_design_12345678',
]

const mcpTools = [{
  name: 'mcp_pencil_batch_design_12345678',
  label: 'MCP · pencil · batch_design',
  description: 'Remote MCP server: pencil. Remote tool name: batch_design.',
}]

test('schema-only cold tools move prompt guidance into their appended schema description', () => {
  const execute = async () => ({ content: [] })
  const tool = schemaOnlyToolDefinition({
    name: 'cold_tool',
    description: 'Cold capability.',
    promptSnippet: 'Cold tool snippet',
    promptGuidelines: ['Use only when explicitly requested.', 'Respect permission boundaries.'],
    execute,
  })
  assert.equal(tool.description, 'Cold capability.\nUse only when explicitly requested.\nRespect permission boundaries.')
  assert.equal(tool.promptSnippet, undefined)
  assert.deepEqual(tool.promptGuidelines, [])
  assert.equal(tool.execute, execute)
})

test('hot tools keep local coding and task progress available without injecting cold schemas', () => {
  assert.deepEqual(hotToolNames(available), [
    'read', 'grep', 'find', 'ls', 'edit', 'write', 'bash', 'generate_visual', 'get_task_list', 'update_task_list', 'discover_tools',
  ])
  assert.deepEqual(selectedToolNames({ availableToolNames: available }), hotToolNames(available))
})

test('promoted cold tools stay active beside the static hot set', () => {
  const promotedToolNames = mergePromotedToolNames({
    availableToolNames: available,
    promotedToolNames: ['web_search'],
    requestedToolNames: ['generate_visual', 'read', 'web_search'],
  })
  assert.deepEqual(promotedToolNames, ['web_search'])
  assert.deepEqual(selectedToolNames({ availableToolNames: available, promotedToolNames }), [
    ...hotToolNames(available),
    'web_search',
  ])
})

test('ordinary local coding requests do not activate cold tools', () => {
  assert.deepEqual(explicitlyRequestedToolNames('修复登录组件并运行测试', { availableToolNames: available, mcpTools }), [])
})

test('explicit web, browser, memory, and Agent requests activate only their cold groups', () => {
  assert.deepEqual(explicitlyRequestedToolNames('请搜索官网的最新版本说明', { availableToolNames: available, mcpTools }), ['web_search'])
  assert.deepEqual(explicitlyRequestedToolNames('打开 https://example.com 并截一张图', { availableToolNames: available, mcpTools }), ['browser_automation'])
  assert.deepEqual(explicitlyRequestedToolNames('生成一张产品海报', { availableToolNames: available, mcpTools }), [])
  assert.deepEqual(explicitlyRequestedToolNames('先给我来个设计图，我看看样式', { availableToolNames: available, mcpTools }), [])
  assert.deepEqual(explicitlyRequestedToolNames('Design a logo for the app', { availableToolNames: available, mcpTools }), [])
  assert.deepEqual(explicitlyRequestedToolNames('记住我的默认语言是中文', { availableToolNames: available, mcpTools }), ['memory_remember'])
  assert.deepEqual(explicitlyRequestedToolNames('如何发版 写入记忆', { availableToolNames: available, mcpTools }), ['memory_remember'])
  assert.equal(isExplicitMemoryRememberRequest('如何发版 写入记忆'), true)
  assert.equal(isExplicitMemoryRememberRequest('修复登录组件并运行测试'), false)
  assert.deepEqual(explicitlyRequestedToolNames('派一个 Agent 并行审查测试', { availableToolNames: available, mcpTools }), [
    'spawn_agent', 'list_agents', 'send_message', 'followup_task', 'wait_agent', 'interrupt_agent',
  ])
})

test('negative mentions do not activate cold tools', () => {
  assert.deepEqual(explicitlyRequestedToolNames('不要使用浏览器，也不要派 Agent', { availableToolNames: available, mcpTools }), [])
  assert.deepEqual(explicitlyRequestedToolNames('不要生成图片，只分析需求', { availableToolNames: available, mcpTools }), [])
  assert.deepEqual(explicitlyRequestedToolNames('Do not use MCP.', { availableToolNames: available, mcpTools }), [])
})

test('explicit MCP requests activate management and remote schemas only when relevant', () => {
  assert.deepEqual(explicitlyRequestedToolNames('列出 MCP 服务', { availableToolNames: available, mcpTools }), ['mcp_list', 'mcp_manage'])
  assert.deepEqual(explicitlyRequestedToolNames('使用 MCP 工具完成设计', { availableToolNames: available, mcpTools }), [
    'mcp_list', 'mcp_manage', 'mcp_pencil_batch_design_12345678',
  ])
  assert.deepEqual(explicitlyRequestedToolNames('调用 batch_design 完成界面', { availableToolNames: available, mcpTools }), [
    'mcp_pencil_batch_design_12345678',
  ])
})
