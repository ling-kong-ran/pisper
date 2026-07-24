import { isVisualContinuationRequest } from '../services/visual-tool-routing.mjs'

const HOT_TOOL_SET = new Set([
  'read',
  'grep',
  'find',
  'ls',
  'edit',
  'write',
  'bash',
  'get_task_list',
  'update_task_list',
  'discover_tools',
])

const MULTI_AGENT_TOOLS = ['spawn_agent', 'list_agents', 'send_message', 'followup_task', 'wait_agent', 'interrupt_agent']

const WEB_SEARCH_REQUEST = /(?:\bweb[_ -]?search\b|\bsearch (?:the )?(?:web|internet|online)\b|\blook (?:it|this|that) up online\b|\blatest (?:release|version|news|documentation)\b|联网|上网|互联网|网上搜索|网络搜索|搜索网页|搜索网站|查找官网|查询官网|最新(?:版本|发布|新闻|资料)|官方(?:文档|资料|来源))/i
const BROWSER_REQUEST = /(?:\bbrowser[_ -]?automation\b|\bopen (?:the )?(?:website|webpage|page|url|link)\b|\bclick (?:the )?(?:page|button|link)\b|\bfill (?:in )?(?:the )?(?:form|field)\b|\bscreenshot\b|\btest (?:the )?(?:website|webpage|page|ui)\b|浏览器|打开.{0,12}(?:网页|网站|链接|页面)|点击.{0,12}(?:页面|按钮|链接)|填写.{0,12}(?:表单|输入框)|网页截图|页面截图|测试.{0,8}(?:网页|网站|页面|界面)|https?:\/\/)/i
const VISUAL_REQUEST = /(?:\bgenerate_visual\b|\b(?:generate|create|draw|design|edit|modify|transform|enhance|upscale|animate|make)\b.{0,24}\b(?:image|picture|photo|illustration|poster|logo|video|animation)\b|\b(?:image|picture|photo|video)\b.{0,24}\b(?:generate|create|edit|modify|transform|enhance|upscale|animate)\b|(?:生成|创建|绘制|画|制作|编辑|修改|转换|增强|放大|修复|设计).{0,16}(?:图片|图像|照片|插画|海报|Logo|logo|视频|动画)|(?:图片|图像|照片|视频).{0,16}(?:生成|创建|编辑|修改|转换|增强|放大|修复))/i
const IMAGE_EDIT_WITH_ATTACHMENT = /(?:去掉|移除|删除|替换|修改|编辑|增强|修复|裁剪|放大|换成|remove|replace|edit|enhance|retouch|crop|upscale)/i
const MEMORY_SEARCH_REQUEST = /(?:\bmemory_search\b|\bsearch (?:your )?memor(?:y|ies)\b|\brecall (?:my|our|the)\b|\bdo you remember\b|搜索星忆|查找星忆|查询星忆|你还记得|还记得我|之前我说过|我的偏好)/i
const MEMORY_REMEMBER_REQUEST = /(?:\bmemory_remember\b|\bremember (?:this|that|my)\b|\bsave (?:this|that) (?:as|to) memor(?:y|ies)\b|\bwrite (?:this|that)? ?(?:to|into) memor(?:y|ies)\b|\bstore (?:this|that) (?:in|as) memor(?:y|ies)\b|记住(?:这个|这点|我的|以后|这)|请记住|帮我记住|记一下|记下来|写入记忆|记入记忆|写入星忆|保存记忆|保存为星忆|加入星忆|创建星忆草稿)/i
const MCP_REQUEST = /(?:\bmcp\b|mcp_list|mcp_manage)/i
const MCP_CALL_REQUEST = /(?:使用|调用|通过|用|call|use|invoke).{0,16}\bmcp\b|\bmcp\b.{0,16}(?:调用|使用|call|use|invoke)/i
const MULTI_AGENT_REQUEST = /(?:\bspawn_agent\b|\blist_agents\b|\bsend_message\b|\bfollowup_task\b|\bwait_agent\b|\binterrupt_agent\b|\bsubagents?\b|\bmulti[- ]agents?\b|\bspawn (?:an? )?agents?\b|\bdelegate (?:this|the task|work)\b|\bparallel agents?\b|子\s*Agent|子\s*agent|派.{0,10}Agent|派.{0,10}agent|委派.{0,10}(?:Agent|agent)|并行.{0,10}(?:Agent|agent)|查看.{0,8}(?:Agent|agent).{0,8}状态|等待.{0,8}(?:Agent|agent)|中断.{0,8}(?:Agent|agent))/i

function normalizedText(value) {
  return String(value || '').toLowerCase().replace(/\s+/g, ' ').trim()
}

function isNegatedAt(text, index) {
  const prefix = text.slice(Math.max(0, index - 32), index)
  return /(?:不要|无需|不需要|禁止|别|不得|请勿|don't|do not|never|without)(?:\s|\S){0,16}$/i.test(prefix)
}

function positiveMatch(text, pattern) {
  const match = text.match(pattern)
  return Boolean(match && !isNegatedAt(text, match.index || 0))
}

function directToolMention(text, toolName) {
  const normalizedName = normalizedText(toolName)
  if (!normalizedName) return false
  const variants = [normalizedName, normalizedName.replaceAll('_', ' ')]
  return variants.some((variant) => {
    const index = text.indexOf(variant)
    return index >= 0 && !isNegatedAt(text, index)
  })
}

function matchingMcpTools(text, mcpTools) {
  const matches = []
  for (const tool of mcpTools || []) {
    const hints = [tool.name, tool.label]
      .flatMap((value) => normalizedText(value).split(/[^\p{L}\p{N}_-]+/u))
      .filter((hint) => hint.length >= 3 && !['mcp', 'remote', 'server', 'tool', 'name'].includes(hint))
    if (hints.some((hint) => {
      const index = text.indexOf(hint)
      return index >= 0 && !isNegatedAt(text, index)
    })) matches.push(tool.name)
  }
  return matches
}

export function schemaOnlyToolDefinition(tool) {
  const guidelines = Array.isArray(tool?.promptGuidelines)
    ? tool.promptGuidelines.map((item) => String(item || '').trim()).filter(Boolean)
    : []
  return {
    ...tool,
    description: [String(tool?.description || '').trim(), ...guidelines].filter(Boolean).join('\n'),
    promptSnippet: undefined,
    promptGuidelines: [],
  }
}

export function schemaOnlyToolDefinitions(tools = []) {
  return tools.map(schemaOnlyToolDefinition)
}

export function hotToolNames(availableToolNames = []) {
  return availableToolNames.filter((name) => HOT_TOOL_SET.has(name))
}

export function isExplicitMemoryRememberRequest(message) {
  return positiveMatch(normalizedText(message), MEMORY_REMEMBER_REQUEST)
}

export function explicitlyRequestedToolNames(message, {
  availableToolNames = [],
  mcpTools = [],
  attachments = [],
  previousRequestedToolNames = [],
} = {}) {
  const text = normalizedText(message)
  const available = new Set(availableToolNames)
  const requested = new Set()
  const add = (...names) => {
    for (const name of names) if (available.has(name)) requested.add(name)
  }

  for (const name of available) {
    if (!HOT_TOOL_SET.has(name) && directToolMention(text, name)) requested.add(name)
  }

  if (positiveMatch(text, WEB_SEARCH_REQUEST)) add('web_search')
  if (positiveMatch(text, BROWSER_REQUEST)) add('browser_automation')
  const hasImageAttachment = attachments.some((attachment) => attachment?.kind === 'image')
  const continuesVisualRequest = previousRequestedToolNames.includes('generate_visual') && isVisualContinuationRequest(text)
  if (positiveMatch(text, VISUAL_REQUEST) || continuesVisualRequest || (hasImageAttachment && positiveMatch(text, IMAGE_EDIT_WITH_ATTACHMENT))) add('generate_visual')
  if (positiveMatch(text, MEMORY_SEARCH_REQUEST)) add('memory_search')
  if (positiveMatch(text, MEMORY_REMEMBER_REQUEST)) add('memory_remember')

  const matchedMcpTools = matchingMcpTools(text, mcpTools)
  add(...matchedMcpTools)
  if (positiveMatch(text, MCP_REQUEST)) {
    add('mcp_list', 'mcp_manage')
    if (positiveMatch(text, MCP_CALL_REQUEST)) add(...mcpTools.map((tool) => tool.name))
  }

  if (positiveMatch(text, MULTI_AGENT_REQUEST)) add(...MULTI_AGENT_TOOLS)
  return [...requested]
}

export function mergePromotedToolNames({
  availableToolNames = [],
  promotedToolNames = [],
  requestedToolNames = [],
} = {}) {
  const available = new Set(availableToolNames)
  return [...new Set([...promotedToolNames, ...requestedToolNames])]
    .filter((name) => available.has(name) && !HOT_TOOL_SET.has(name))
}

export function selectedToolNames({
  availableToolNames = [],
  promotedToolNames = [],
  requestedToolNames = [],
  goalToolNames = [],
  goalActive = false,
} = {}) {
  const available = new Set(availableToolNames)
  const names = new Set(hotToolNames(availableToolNames))
  for (const name of promotedToolNames) if (available.has(name)) names.add(name)
  for (const name of requestedToolNames) if (available.has(name)) names.add(name)
  if (goalActive) for (const name of goalToolNames) if (available.has(name)) names.add(name)
  return [...names]
}
