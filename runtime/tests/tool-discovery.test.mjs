import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createToolDiscoveryTool,
  searchOptionalTools,
  TOOL_DISCOVERY_NAME,
} from '../tools/app/tool-discovery.mjs'
import { createRuntimeToolGateway } from '../runtime/tool-gateway-runtime.mjs'

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
  {
    name: 'skill_create',
    label: 'Skill Create',
    description: 'Create a reusable Agent Skill in the project or global skills directory.',
    active: false,
  },
  {
    name: 'plugin_create',
    label: 'Plugin Create',
    description: 'Create a reusable Pisper plugin and new Agent tools.',
    active: false,
  },
]

const computerUseTools = [
  {
    name: 'computer_use',
    label: 'Computer Use',
    description: 'See the screen and control the mouse and keyboard.',
    active: false,
  },
  {
    name: 'web_search',
    label: 'Web Search',
    description: 'Search the web.',
    active: false,
  },
]

test('computer-use discovery aliases find the tool from natural tasks', () => {
  for (const query of ['控制电脑并点击窗口', '屏幕截图', '鼠标键盘', 'computer use'])
    assert.equal(searchOptionalTools(computerUseTools, query, 1)[0]?.name, 'computer_use', query)
})

test('optional tool search understands capability aliases and exact dynamic tool labels', () => {
  assert.equal(searchOptionalTools(tools, '帮我生图', 1)[0].name, 'generate_visual')
  // Bot 的通知工具：中文说法与平台名都搜得到。
  const botTools = [
    {
      name: 'bot_notify',
      label: 'Notify the user on their chat apps',
      description: 'Send a message',
    },
    { name: 'bot_schedule', label: 'Schedule a message to yourself', description: 'Schedule' },
  ]
  for (const query of ['通过了 QQ 上通知我', 'notify push message plugin user', '推送到飞书群'])
    assert.equal(searchOptionalTools(botTools, query, 1)[0]?.name, 'bot_notify', query)
  assert.equal(
    searchOptionalTools(tools, 'search the latest information online', 1)[0].name,
    'web_search',
  )
  assert.equal(
    searchOptionalTools(tools, 'MCP fixture echo', 1)[0].name,
    'mcp_fixture_echo_12345678',
  )
  assert.equal(searchOptionalTools(tools, '创建技能', 1)[0].name, 'skill_create')
  assert.equal(searchOptionalTools(tools, '生成工具', 1)[0].name, 'plugin_create')
  assert.equal(
    searchOptionalTools(
      [
        ...tools,
        {
          name: 'mobile_device',
          label: 'Mobile Device',
          description: 'Use approved phone capabilities.',
          active: false,
        },
      ],
      '查看当前设备系统内存总量和可用内存',
      1,
    )[0].name,
    'mobile_device',
  )
})

test('discover_tools presents friendly computer-use labels while preserving exact names', async () => {
  const tool = createToolDiscoveryTool({ listTools: () => computerUseTools })
  const result = await tool.execute(
    'discover-computer-use',
    { query: '点击窗口', limit: 1 },
    new AbortController().signal,
  )
  assert.match(result.content[0].text, /Computer Use \[computer_use\]/)
  assert.match(result.content[0].text, /call through call_tool/)
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
  assert.match(result.content[0].text, /inactive: call through call_tool/)
})

test('discover_tools describes direct invocation for active tools', async () => {
  const tool = createToolDiscoveryTool({
    listTools: () =>
      tools.map((item) => (item.name === 'skill_create' ? { ...item, active: true } : item)),
  })
  const result = await tool.execute(
    'discover-active',
    { query: '创建技能', limit: 1 },
    new AbortController().signal,
  )
  assert.equal(result.details.matches[0].name, 'skill_create')
  assert.match(result.content[0].text, /active: call this tool directly/)
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

const browserTool = {
  name: 'browser_automation',
  label: 'Browser Automation',
  description: 'Navigate, inspect, click, type, wait, and capture screenshots.',
  active: false,
}

test('optional tool search understands natural CJK phrasing with inserted particles', () => {
  // 「截个图」插入虚字,连续子串匹配会漏;跳跃二元组应兜住 alias「网页截图」
  assert.equal(
    searchOptionalTools([...tools, browserTool], '帮我截个图', 1)[0].name,
    'browser_automation',
  )
  assert.equal(
    searchOptionalTools([...tools, browserTool], '想打开网页点一下', 1)[0].name,
    'browser_automation',
  )
})

test('CJK fuzzy scoring never outranks exact name matches', () => {
  const results = searchOptionalTools([browserTool, ...tools], 'web_search', 2)
  assert.equal(results[0].name, 'web_search')
})

test('discover_tools includes compact parameter signatures', async () => {
  const toolWithSchema = {
    name: 'web_search',
    label: 'Web Search',
    description: 'Search the public web.',
    active: false,
    required: ['query'],
    parameters: {
      type: 'object',
      properties: {
        query: { type: 'string' },
        limit: { type: 'integer' },
      },
      required: ['query'],
    },
  }
  const tool = createToolDiscoveryTool({ listTools: () => [toolWithSchema] })
  const result = await tool.execute(
    'discover-sig',
    { query: 'web_search', limit: 1 },
    new AbortController().signal,
  )
  assert.match(result.content[0].text, /params: query: string, limit\?: integer/)
})

test('discover_tools expands literal action values instead of reporting any', async () => {
  const tool = createToolDiscoveryTool({
    listTools: () => [
      {
        name: 'mobile_device',
        label: 'Mobile Device',
        description: 'Use approved phone capabilities.',
        active: false,
        required: ['action'],
        parameters: {
          type: 'object',
          properties: {
            action: {
              anyOf: [{ const: 'get_device_info' }, { const: 'get_capabilities' }],
            },
          },
          required: ['action'],
        },
      },
    ],
  })
  const result = await tool.execute(
    'discover-actions',
    { query: 'mobile device', limit: 1 },
    new AbortController().signal,
  )
  assert.match(result.content[0].text, /params: action: "get_device_info" \| "get_capabilities"/)
  assert.doesNotMatch(result.content[0].text, /action: any/)
})

test('discover_tools spells out the name to pass to call_tool, not just the display label', async () => {
  const tool = createToolDiscoveryTool({
    listTools: () => [
      { name: 'browser_automation', label: 'Browser Control', description: 'Browser' },
    ],
  })
  const result = await tool.execute('discover', { query: 'browser' })
  assert.match(result.content[0].text, /call through call_tool with name "browser_automation"/)
})

function gatewayFixture(extra = []) {
  const calls = []
  const authorized = []
  const execute = (name) => async () => {
    calls.push(name)
    return { content: [] }
  }
  const tools = new Map(
    [
      { name: 'browser_automation', label: 'Browser Control' },
      { name: 'computer_use', label: 'Computer Use' },
      ...extra,
    ].map((tool) => [tool.name, { parameters: {}, execute: execute(tool.name), ...tool }]),
  )
  const gateway = createRuntimeToolGateway({
    tools,
    sessionId: 's',
    authorize: async (input) => {
      authorized.push(input.toolName)
      return input.toolName === 'computer_use' ? { block: true, reason: 'blocked' } : undefined
    },
  })
  return { gateway, calls, authorized }
}

test('call_tool accepts a display label that maps to exactly one tool, and authorizes the real name', async () => {
  for (const name of [
    'browser_automation',
    'Browser Control',
    'browser control',
    'Browser Control [browser_automation]',
  ]) {
    const { gateway, calls, authorized } = gatewayFixture()
    const result = await gateway.execute('call', { name })
    assert.deepEqual(calls, ['browser_automation'], name)
    assert.deepEqual(authorized, ['browser_automation'], name)
    assert.equal(result.details.gatewayToolName, 'browser_automation', name)
  }
  // 用显示名也绕不过审批。
  const { gateway, calls } = gatewayFixture()
  await assert.rejects(gateway.execute('call', { name: 'Computer Use' }), /blocked/)
  assert.deepEqual(calls, [])
})

test('call_tool still refuses unknown or ambiguous names without guessing', async () => {
  const { gateway } = gatewayFixture([{ name: 'other_browser', label: 'Browser Control' }])
  await assert.rejects(
    gateway.execute('call', { name: 'Browser Control' }),
    /Optional tool is unavailable: Browser Control\. Use the exact name/,
  )
  await assert.rejects(gateway.execute('call', { name: 'browser' }), /unavailable/)
  await assert.rejects(gateway.execute('call', { name: 'Nope [missing]' }), /unavailable/)
})
