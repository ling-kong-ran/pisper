import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createToolDiscoveryTool,
  searchOptionalTools,
  TOOL_DISCOVERY_NAME,
} from '../tools/app/tool-discovery.mjs'

const tools = [
  {
    name: 'web_search',
    label: 'Web Search',
    description: 'Search the public web for current information.',
    active: false,
  },
  {
    name: 'generate_visual',
    label: 'Visual Generate',
    description: 'Generate or edit images and generate video.',
    active: false,
  },
  {
    name: 'mcp_fixture_echo_12345678',
    label: 'MCP fixture echo',
    description: 'Echo fixture text through a remote MCP service.',
    active: false,
  },
]

test('optional tool search understands capability aliases and exact dynamic tool labels', () => {
  assert.equal(searchOptionalTools(tools, '帮我生图', 1)[0].name, 'generate_visual')
  assert.equal(
    searchOptionalTools(tools, 'search the latest information online', 1)[0].name,
    'web_search',
  )
  assert.equal(
    searchOptionalTools(tools, 'MCP fixture echo', 1)[0].name,
    'mcp_fixture_echo_12345678',
  )
})

test('discover_tools returns stable gateway matches without activating schemas', async () => {
  const tool = createToolDiscoveryTool({ listTools: () => tools })
  assert.equal(tool.name, TOOL_DISCOVERY_NAME)
  const result = await tool.execute(
    'discover-1',
    { query: 'generate an image', limit: 1 },
    new AbortController().signal,
  )
  assert.deepEqual(
    result.details.matches.map((match) => match.name),
    ['generate_visual'],
  )
  assert.equal(result.details.activated, undefined)
  assert.match(result.content[0].text, /Call one through call_tool/)
})

test('discover_tools returns matches without any activation callback', async () => {
  const tool = createToolDiscoveryTool({ listTools: () => tools })
  const result = await tool.execute(
    'discover-2',
    { query: 'browser web search', limit: 1 },
    new AbortController().signal,
  )
  assert.equal(result.details.matches[0].name, 'web_search')
  assert.equal(result.details.activated, undefined)
})
