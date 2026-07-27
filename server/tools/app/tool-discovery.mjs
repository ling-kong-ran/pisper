import { defineTool } from '@earendil-works/pi-coding-agent'
import { Type } from 'typebox'

export const TOOL_DISCOVERY_NAME = 'discover_tools'

const TOOL_ALIASES = {
  web_search: ['web search', 'internet search', 'online search', '联网', '上网', '网络搜索', '网页搜索', '官网', '最新资料'],
  browser_automation: ['browser', 'browser automation', 'screenshot', '浏览器', '打开网页', '点击网页', '网页截图', '页面操作', '自动化'],
  generate_visual: ['visual generation', 'image generation', 'video generation', 'image editing', '生图', '画图', '图片生成', '图像生成', '视频生成', '图片编辑', '视觉生成'],
  memory_search: ['memory search', 'recall', '记忆搜索', '搜索记忆', '星忆', '回忆'],
  memory_remember: ['remember', 'save memory', '记住', '保存记忆', '写入记忆', '星忆'],
  mcp_list: ['mcp', 'mcp services', 'mcp tools', 'mcp 服务', 'mcp 工具'],
  mcp_manage: ['mcp configuration', 'configure mcp', 'mcp 配置', '管理 mcp', '添加 mcp'],
  spawn_agent: ['subagent', 'delegate', 'parallel agent', '子 agent', '子agent', '委派', '并行 agent'],
  list_agents: ['agent status', 'subagent status', 'agent 状态', '子agent状态'],
  send_message: ['message agent', 'steer agent', '给 agent 发消息', '补充 agent 信息'],
  followup_task: ['agent followup', 'continue agent', 'agent 后续任务', '让 agent 继续'],
  wait_agent: ['wait agent', '等待 agent', '等待子agent'],
  interrupt_agent: ['stop agent', 'interrupt agent', '停止 agent', '中断 agent'],
}

function normalized(value) {
  return String(value || '').toLowerCase().replace(/[_-]+/g, ' ').replace(/\s+/g, ' ').trim()
}

function queryTerms(query) {
  return normalized(query).split(/[^\p{L}\p{N}]+/u).filter((term) => term.length >= 2)
}

function characterOverlap(query, value) {
  const queryCharacters = new Set([...query].filter((character) => /[\p{L}\p{N}]/u.test(character)))
  if (queryCharacters.size < 2) return 0
  const valueCharacters = new Set([...value])
  let matches = 0
  for (const character of queryCharacters) if (valueCharacters.has(character)) matches += 1
  return matches / queryCharacters.size
}

function toolSearchText(tool) {
  return normalized([
    tool.name,
    tool.label,
    tool.description,
    ...(TOOL_ALIASES[tool.name] || []),
  ].filter(Boolean).join(' '))
}

function scoreTool(tool, query) {
  const normalizedQuery = normalized(query)
  const normalizedName = normalized(tool.name)
  const normalizedLabel = normalized(tool.label)
  const haystack = toolSearchText(tool)
  if (!normalizedQuery) return 1
  let score = 0
  if (normalizedQuery === normalizedName || normalizedQuery === normalizedLabel) score += 240
  if (normalizedName.includes(normalizedQuery) || normalizedLabel.includes(normalizedQuery)) score += 120
  if (haystack.includes(normalizedQuery)) score += 80
  for (const alias of TOOL_ALIASES[tool.name] || []) {
    const normalizedAlias = normalized(alias)
    if (normalizedAlias && (normalizedQuery.includes(normalizedAlias) || normalizedAlias.includes(normalizedQuery))) score += 72
  }
  for (const term of queryTerms(normalizedQuery)) {
    if (normalizedName.includes(term)) score += 36
    else if (normalizedLabel.includes(term)) score += 30
    else if (haystack.includes(term)) score += 18
  }
  const overlap = characterOverlap(normalizedQuery, haystack)
  if (overlap >= 0.8) score += 24
  else if (overlap >= 0.6) score += 12
  return score
}

export function searchOptionalTools(tools, query, limit = 3) {
  const boundedLimit = Math.max(1, Math.min(5, Number(limit) || 3))
  return (tools || [])
    .map((tool) => ({ ...tool, score: scoreTool(tool, query) }))
    .filter((tool) => tool.score > 0)
    .sort((left, right) => right.score - left.score || left.name.localeCompare(right.name))
    .slice(0, boundedLimit)
    .map(({ score: _score, ...tool }) => tool)
}

function formatMatch(tool, activated) {
  const state = activated ? 'activated' : tool.active ? 'already active' : 'available'
  return `- ${tool.name} (${state}): ${tool.description || tool.label || 'Optional tool'}`
}

export function createToolDiscoveryTool({ listTools, activateTools }) {
  return defineTool({
    name: TOOL_DISCOVERY_NAME,
    label: 'Discover Tools',
    description: 'Search Pisper optional capabilities by task or intent and activate the most relevant tool schemas for the current session.',
    promptSnippet: 'Search and activate optional tools when the currently active tools do not cover the task',
    promptGuidelines: [
      'Use discover_tools when the task may require an optional capability whose tool schema is not currently active.',
      'Search by the capability you need, not by guessing an unavailable tool call.',
      'When discover_tools activates a tool, use that activated tool on the next step instead of merely describing it.',
      'Do not use discover_tools when the currently active tools already cover the task.',
    ],
    parameters: Type.Object({
      query: Type.String({ minLength: 1, maxLength: 240, description: 'Capability, task, or tool name to search for' }),
      limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 5, description: 'Maximum number of matching tools to return and activate; default 3' })),
      activate: Type.Optional(Type.Boolean({ description: 'Whether to add matching tools to the current session context; default true' })),
    }),
    async execute(_toolCallId, params) {
      const tools = await listTools?.() || []
      const matches = searchOptionalTools(tools, params.query, params.limit || 3)
      if (!matches.length) {
        return {
          content: [{ type: 'text', text: `No optional tools matched: ${params.query}` }],
          details: { query: params.query, matches: [], activated: [] },
        }
      }
      const shouldActivate = params.activate !== false
      const activation = shouldActivate
        ? await activateTools?.(matches.map((tool) => tool.name))
        : { activatedToolNames: [] }
      const activatedToolNames = activation?.activatedToolNames || []
      const activated = new Set(activatedToolNames)
      const text = [
        shouldActivate
          ? 'Matching optional tools were discovered. Newly available schemas are active for the current session:'
          : 'Matching optional tools:',
        ...matches.map((tool) => formatMatch(tool, activated.has(tool.name))),
      ].join('\n')
      return {
        content: [{ type: 'text', text }],
        details: {
          query: params.query,
          matches,
          activated: activatedToolNames,
          promotedToolNames: activation?.promotedToolNames || [],
        },
      }
    },
  })
}
