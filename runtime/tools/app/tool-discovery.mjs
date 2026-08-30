import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'

// 工具发现工具：让模型按别名/关键词发现可选工具（discover_tools），
// 配合 call_tool 网关实现按需激活。
export const TOOL_DISCOVERY_NAME = 'discover_tools'

const TOOL_ALIASES = {
  web_search: [
    'web search',
    'internet search',
    'online search',
    '联网',
    '上网',
    '网络搜索',
    '网页搜索',
    '官网',
    '最新资料',
  ],
  browser_automation: [
    'browser',
    'browser automation',
    'screenshot',
    '浏览器',
    '打开网页',
    '点击网页',
    '网页截图',
    '页面操作',
    '自动化',
  ],
  generate_visual: [
    'visual generation',
    'image generation',
    'video generation',
    'image editing',
    '生图',
    '画图',
    '图片生成',
    '图像生成',
    '视频生成',
    '图片编辑',
    '视觉生成',
  ],
  mobile_device: [
    'mobile device',
    'phone',
    'device status',
    'system information',
    'memory',
    'RAM',
    '移动设备',
    '手机',
    '手机设备',
    '设备状态',
    '设备信息',
    '系统信息',
    '系统内存',
    '内存',
  ],
  memory_search: ['memory search', 'recall', '记忆搜索', '搜索记忆', '星忆', '回忆'],
  memory_remember: ['remember', 'save memory', '记住', '保存记忆', '写入记忆', '星忆'],
  mcp_list: ['mcp', 'mcp services', 'mcp tools', 'mcp 服务', 'mcp 工具'],
  mcp_manage: ['mcp configuration', 'configure mcp', 'mcp 配置', '管理 mcp', '添加 mcp'],
  skill_create: [
    'skill',
    'create skill',
    'agent skill',
    'skill.md',
    '技能',
    '创建技能',
    '编写技能',
    '可复用能力',
  ],
  plugin_create: [
    'plugin',
    'create plugin',
    'agent tool',
    '插件',
    '创建插件',
    '创建工具',
    '生成工具',
    '可复用工具',
  ],
  spawn_agent: [
    'subagent',
    'delegate',
    'parallel agent',
    '子 agent',
    '子agent',
    '委派',
    '并行 agent',
  ],
  list_agents: ['agent status', 'subagent status', 'agent 状态', '子agent状态'],
  send_message: ['message agent', 'steer agent', '给 agent 发消息', '补充 agent 信息'],
  followup_task: ['agent followup', 'continue agent', 'agent 后续任务', '让 agent 继续'],
  wait_agent: ['wait agent', '等待 agent', '等待子agent'],
  interrupt_agent: ['stop agent', 'interrupt agent', '停止 agent', '中断 agent'],
}

function normalized(value) {
  return String(value || '')
    .toLowerCase()
    .replace(/[_-]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

function queryTerms(query) {
  return normalized(query)
    .split(/[^\p{L}\p{N}]+/u)
    .filter((term) => term.length >= 2)
}

function characterOverlap(query, value) {
  const queryCharacters = new Set([...query].filter((character) => /[\p{L}\p{N}]/u.test(character)))
  if (queryCharacters.size < 2) return 0
  const valueCharacters = new Set([...value])
  let matches = 0
  for (const character of queryCharacters) if (valueCharacters.has(character)) matches += 1
  return matches / queryCharacters.size
}

// CJK 范围:平/片假名 +  ext-A + 统一表意文字 + 兼容表意文字
const CJK_PATTERN = /[\u3040-\u30ff\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]/u

// 跳跃二元组:中文口语常插入虚字(「截个图」vs「截图」),
// 连续子串匹配会漏,有序字符对(允许跳过中间字)能兜住这类自然说法。
function skipBigrams(value, maxLength = 24) {
  const characters = [...value]
    .filter((character) => /[\p{L}\p{N}]/u.test(character))
    .slice(0, maxLength)
  const pairs = new Set()
  for (let i = 0; i < characters.length; i++)
    for (let j = i + 1; j < characters.length; j++) pairs.add(characters[i] + characters[j])
  return pairs
}

function toolSearchText(tool) {
  return normalized(
    [tool.name, tool.label, tool.description, ...(TOOL_ALIASES[tool.name] || [])]
      .filter(Boolean)
      .join(' '),
  )
}

function scoreTool(tool, query) {
  const normalizedQuery = normalized(query)
  const normalizedName = normalized(tool.name)
  const normalizedLabel = normalized(tool.label)
  const haystack = toolSearchText(tool)
  if (!normalizedQuery) return 1
  let score = 0
  if (normalizedQuery === normalizedName || normalizedQuery === normalizedLabel) score += 240
  if (normalizedName.includes(normalizedQuery) || normalizedLabel.includes(normalizedQuery))
    score += 120
  if (haystack.includes(normalizedQuery)) score += 80
  for (const alias of TOOL_ALIASES[tool.name] || []) {
    const normalizedAlias = normalized(alias)
    if (
      normalizedAlias &&
      (normalizedQuery.includes(normalizedAlias) || normalizedAlias.includes(normalizedQuery))
    )
      score += 72
  }
  for (const term of queryTerms(normalizedQuery)) {
    if (normalizedName.includes(term)) score += 36
    else if (normalizedLabel.includes(term)) score += 30
    else if (haystack.includes(term)) score += 18
  }
  const overlap = characterOverlap(normalizedQuery, haystack)
  if (overlap >= 0.8) score += 24
  else if (overlap >= 0.6) score += 12
  // CJK 口语容错:只在大于等于一个 CJK 字符时启用,避免给英文查询引入噪声;
  // 得分封顶 56,保证它只能「兜底」,不能压过精确名/子串/别名命中。
  if (CJK_PATTERN.test(normalizedQuery)) {
    const compactHaystack = haystack.replace(/[^\p{L}\p{N}]+/gu, '')
    const pairs = skipBigrams(normalizedQuery)
    if (pairs.size) {
      let hits = 0
      for (const pair of pairs) if (compactHaystack.includes(pair)) hits += 1
      score += Math.round((hits / pairs.size) * 56)
    }
  }
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

// 紧凑参数签名:模型经 call_tool 调用前就能看到参数形状,
// 避免凭工具名盲猜参数导致的首轮调用失败。
function formatSchema(schema) {
  if (schema && Object.prototype.hasOwnProperty.call(schema, 'const')) {
    return JSON.stringify(schema.const)
  }
  if (Array.isArray(schema?.enum))
    return schema.enum.map((value) => JSON.stringify(value)).join(' | ')
  if (Array.isArray(schema?.anyOf)) {
    return schema.anyOf
      .map((item) => formatSchema(item))
      .filter(Boolean)
      .join(' | ')
  }
  return Array.isArray(schema?.type) ? schema.type.join('|') : schema?.type || 'any'
}

function formatSignature(tool) {
  const properties = tool.parameters?.properties
  if (!properties || typeof properties !== 'object') return ''
  const required = new Set(Array.isArray(tool.required) ? tool.required : [])
  const parts = []
  for (const [key, schema] of Object.entries(properties).slice(0, 8)) {
    parts.push(`${key}${required.has(key) ? '' : '?'}: ${formatSchema(schema)}`)
  }
  return parts.length ? `; params: ${parts.join(', ')}` : ''
}

function formatMatch(tool) {
  const invocation = tool.active
    ? '; active: call this tool directly'
    : '; inactive: call through call_tool'
  return `- ${tool.name}: ${tool.description || tool.label || 'Optional tool'}${formatSignature(tool)}${invocation}`
}

export function createToolDiscoveryTool({ listTools }) {
  return defineTool({
    name: TOOL_DISCOVERY_NAME,
    label: 'Discover Tools',
    // 描述里直接枚举常见能力，避免模型不知道有哪些可选工具（如生图）可用。
    description:
      'Find optional tools by capability: image/video generation & editing (generate_visual), web search, browser automation, mobile device control, memory, MCP, plugins, and more. Call results through call_tool.',
    promptSnippet:
      'Find optional tools (image/video generation, web, device, memory, MCP) by capability',
    promptGuidelines: [
      'Search by capability, then call the exact result through call_tool.',
      'When the user asks for generated images/videos, device actions, web info, or other app capabilities, discover and call the matching tool instead of claiming it is unavailable.',
    ],
    parameters: Type.Object({
      query: Type.String({
        minLength: 1,
        maxLength: 240,
        description: 'Capability or task to find',
      }),
      limit: Type.Optional(
        Type.Integer({
          minimum: 1,
          maximum: 5,
          description: 'Maximum matches; default 3',
        }),
      ),
    }),
    async execute(_toolCallId, params) {
      const tools = (await listTools?.()) || []
      const matches = searchOptionalTools(tools, params.query, params.limit || 3)
      if (!matches.length) {
        return {
          content: [{ type: 'text', text: `No optional tools matched: ${params.query}` }],
          details: { query: params.query, matches: [] },
        }
      }
      const text = [
        'Matching tools. Call active tools directly; call inactive tools through call_tool with their exact names:',
        ...matches.map((tool) => formatMatch(tool)),
      ].join('\n')
      return {
        content: [{ type: 'text', text }],
        details: { query: params.query, matches },
      }
    },
  })
}
