// 上下文各部分占了多少：系统提示词、项目说明、技能、工具定义、MCP、对话、工具结果。
// 模型只返回总量，不告诉每部分各占多少，所以各部分是估算值；有模型返回的总量时按它校准，
// 让各部分加起来等于真实用量。输入是会话消息（系统提示词和工具定义都以系统消息的形式记在转录里），
// 驻留和冷会话走同一套，纯函数，不读文件。

const WIDE_CHARACTER_START = 0x2e80
const CHARS_PER_TOKEN = 4
// 中日韩文字平均每字一个多 token，按英文 4 字符一个算会把中文说明低估好几倍。
const TOKENS_PER_WIDE_CHARACTER = 1.5
const TOKENS_PER_IMAGE = 1200
const MCP_DESCRIPTION = /Remote MCP server: (.+?)\.(?:\s|$)/
const TOOL_GATEWAY = 'call_tool'

export const CONTEXT_CATEGORIES = Object.freeze([
  'system',
  'project',
  'skills',
  'tools',
  'mcp',
  'messages',
  'toolResults',
])

export function estimateTextTokens(text) {
  const value = String(text ?? '')
  let wide = 0
  for (const character of value) {
    if (character.codePointAt(0) >= WIDE_CHARACTER_START) wide += 1
  }
  return Math.ceil((value.length - wide) / CHARS_PER_TOKEN + wide * TOKENS_PER_WIDE_CHARACTER)
}

function contentTokens(content) {
  if (typeof content === 'string') return estimateTextTokens(content)
  if (!Array.isArray(content)) return 0
  let tokens = 0
  for (const block of content) {
    if (block?.type === 'text') tokens += estimateTextTokens(block.text)
    else if (block?.type === 'image') tokens += TOKENS_PER_IMAGE
  }
  return tokens
}

function toolTokens(tool) {
  return estimateTextTokens(
    JSON.stringify({
      name: tool?.name,
      description: tool?.description,
      parameters: tool?.parameters,
    }),
  )
}

// 系统消息是增量：sections 里 null 表示删掉这一段，toolsAdded / toolsRemoved 是工具表的变化。
function collectPrompt(messages) {
  const sections = new Map()
  const tools = new Map()
  for (const message of Array.isArray(messages) ? messages : []) {
    if (message?.role !== 'system') continue
    if (typeof message.content === 'string' && message.content)
      sections.set('preamble', message.content)
    for (const [name, content] of Object.entries(message.sections ?? {})) {
      if (content) sections.set(name, content)
      else sections.delete(name)
    }
    for (const tool of message.toolsAdded ?? []) if (tool?.name) tools.set(tool.name, tool)
    for (const name of message.toolsRemoved ?? [])
      tools.delete(typeof name === 'string' ? name : name?.name)
  }
  return { sections, tools }
}

// 对话本身。调用工具时写的参数和工具返回的内容是一回事的两半，算在一起；
// 调的是 MCP 工具（直接调用或经 call_tool）就归到 MCP 那一行，否则定义不进上下文时 MCP 永远显示 0。
function collectConversation(messages, ownerOf) {
  let conversation = 0
  let conversationCount = 0
  let results = 0
  let resultCount = 0
  const mcpCalls = new Map()
  const mcpUsage = new Map()
  const addUsage = (server, tokens, calls = 0) => {
    const entry = mcpUsage.get(server) ?? { tokens: 0, calls: 0 }
    entry.tokens += tokens
    entry.calls += calls
    mcpUsage.set(server, entry)
  }
  for (const message of Array.isArray(messages) ? messages : []) {
    switch (message?.role) {
      case 'system':
        break
      case 'assistant':
        conversationCount += 1
        for (const block of Array.isArray(message.content) ? message.content : []) {
          if (block?.type === 'text') conversation += estimateTextTokens(block.text)
          else if (block?.type === 'thinking') conversation += estimateTextTokens(block.thinking)
          else if (block?.type === 'toolCall') {
            const tokens = estimateTextTokens(
              `${block.name}${JSON.stringify(block.arguments ?? {})}`,
            )
            const target = block.name === TOOL_GATEWAY ? block.arguments?.name : block.name
            const server = ownerOf(target)
            if (!server) {
              results += tokens
              continue
            }
            addUsage(server, tokens, 1)
            if (block.id) mcpCalls.set(block.id, server)
          }
        }
        break
      case 'toolResult': {
        const tokens = contentTokens(message.content)
        const server = mcpCalls.get(message.toolCallId)
        if (server) addUsage(server, tokens)
        else {
          results += tokens
          resultCount += 1
        }
        break
      }
      case 'bashExecution':
        results += estimateTextTokens(`${message.command ?? ''}${message.output ?? ''}`)
        resultCount += 1
        break
      case 'branchSummary':
      case 'compactionSummary':
        conversation += estimateTextTokens(message.summary)
        break
      default:
        conversation += contentTokens(message?.content)
        if (message?.role === 'user') conversationCount += 1
    }
  }
  return { conversation, conversationCount, results, resultCount, mcpUsage }
}

// 把估算值按真实总量等比校准。四舍五入的差额给最大的一项，保证加起来正好等于总量。
function calibrate(categories, total) {
  const estimated = categories.reduce((sum, category) => sum + category.tokens, 0)
  if (!estimated || !total) return categories
  const factor = total / estimated
  const scaled = categories.map((category) => ({
    ...category,
    tokens: Math.round(category.tokens * factor),
    ...(category.servers
      ? {
          servers: category.servers.map((server) => ({
            ...server,
            tokens: Math.round(server.tokens * factor),
          })),
        }
      : {}),
  }))
  const difference = total - scaled.reduce((sum, category) => sum + category.tokens, 0)
  const largest = scaled.reduce((best, category) =>
    category.tokens > best.tokens ? category : best,
  )
  largest.tokens += difference
  return scaled
}

/**
 * @param {{ messages?: unknown[], systemPrompt?: string, activeTools?: unknown[], usage?: Record<string, unknown> | null, mcpAvailable?: { name: string, toolNames: string[] }[] }} input
 * systemPrompt 与 activeTools 是驻留会话的兜底：还没发过消息时转录里没有系统消息。
 * mcpAvailable 是已连上的 MCP 服务和它们的工具在会话里的名字：工具按需调用，没用到的不占上下文。
 */
export function buildContextBreakdown({
  messages,
  systemPrompt,
  activeTools,
  usage,
  mcpAvailable,
} = {}) {
  const { sections, tools } = collectPrompt(messages)
  if (!sections.size && systemPrompt) sections.set('preamble', systemPrompt)
  if (!tools.size) for (const tool of activeTools ?? []) if (tool?.name) tools.set(tool.name, tool)

  let system = 0
  let project = 0
  let skills = 0
  for (const [name, content] of sections) {
    const tokens = estimateTextTokens(content)
    if (name === 'project_context') project += tokens
    else if (name === 'skills') skills += tokens
    else system += tokens
  }

  // 工具在会话里的名字 → 所属 MCP 服务：已连上的服务和转录里已加载的定义都算，服务后来断开也认得出。
  const owners = new Map()
  for (const server of mcpAvailable ?? [])
    for (const name of server.toolNames ?? []) owners.set(name, server.name)
  let builtinTokens = 0
  let builtinCount = 0
  const servers = new Map()
  const serverEntry = (name) => {
    const entry = servers.get(name) ?? { name, tools: 0, calls: 0, tokens: 0 }
    servers.set(name, entry)
    return entry
  }
  for (const tool of tools.values()) {
    const tokens = toolTokens(tool)
    const server = MCP_DESCRIPTION.exec(String(tool?.description ?? ''))?.[1]
    if (!server) {
      builtinTokens += tokens
      builtinCount += 1
      continue
    }
    owners.set(tool.name, server)
    const entry = serverEntry(server)
    entry.tools += 1
    entry.tokens += tokens
  }
  const collected = collectConversation(messages, (name) => owners.get(name))
  for (const [name, usage_] of collected.mcpUsage) {
    const entry = serverEntry(name)
    entry.tokens += usage_.tokens
    entry.calls += usage_.calls
  }
  const mcpServers = [...servers.values()].sort((left, right) => right.tokens - left.tokens)

  const reported = Number(usage?.tokens)
  const totalKnown = usage?.tokens != null && Number.isFinite(reported) && reported > 0
  const categories = calibrate(
    [
      { id: 'system', tokens: system },
      { id: 'project', tokens: project },
      { id: 'skills', tokens: skills },
      { id: 'tools', tokens: builtinTokens, count: builtinCount },
      {
        id: 'mcp',
        tokens: mcpServers.reduce((sum, server) => sum + server.tokens, 0),
        count: mcpServers.reduce((sum, server) => sum + server.tools, 0),
        calls: mcpServers.reduce((sum, server) => sum + server.calls, 0),
        // 连上了但定义没进上下文的工具数：界面要说清楚是“没占”，不是“没统计到”。
        deferred: (mcpAvailable ?? []).reduce((sum, server) => {
          const loaded = servers.get(server.name)?.tools ?? 0
          return sum + Math.max(0, (server.toolNames?.length ?? 0) - loaded)
        }, 0),
        servers: mcpServers,
      },
      { id: 'messages', tokens: collected.conversation, count: collected.conversationCount },
      { id: 'toolResults', tokens: collected.results, count: collected.resultCount },
    ],
    totalKnown ? reported : 0,
  )
  const usedTokens = categories.reduce((sum, category) => sum + category.tokens, 0)
  const contextWindow = Math.max(0, Number(usage?.contextWindow) || 0)
  return {
    version: 1,
    contextWindow,
    usedTokens,
    freeTokens: Math.max(0, contextWindow - usedTokens),
    // 总量来自模型返回的用量时为 true；否则连总量也是估算（还没有响应，或刚压缩过）。
    totalKnown,
    // 还没发过消息的冷会话：转录里没有系统提示词和工具，这几项是未知而不是 0。
    prefixKnown: sections.size > 0 || tools.size > 0,
    compactAtPercent: usage?.autoCompactEnabled ? Number(usage.compactAtPercent) || null : null,
    categories,
  }
}
