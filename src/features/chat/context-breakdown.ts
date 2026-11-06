// 上下文占比的数据：Runtime 响应先归一化，界面只读这里的形状。
export const CONTEXT_CATEGORY_IDS = [
  'system',
  'project',
  'skills',
  'tools',
  'mcp',
  'messages',
  'toolResults',
] as const

export type ContextCategoryId = (typeof CONTEXT_CATEGORY_IDS)[number]

export type ContextCategory = {
  id: ContextCategoryId
  tokens: number
  count: number | null
  // MCP：连上了但还没用到的工具数，它们不占上下文。
  deferred: number
  // MCP：调用次数（调用的参数和结果算在 MCP 这一行）。
  calls: number
  servers: { name: string; tools: number; calls: number; tokens: number }[]
}

export type ContextBreakdown = {
  contextWindow: number
  usedTokens: number
  freeTokens: number
  totalKnown: boolean
  prefixKnown: boolean
  compactAtPercent: number | null
  categories: ContextCategory[]
}

type Fields = Record<string, unknown>

function record(value: unknown): Fields {
  return value && typeof value === 'object' ? (value as Fields) : {}
}

function tokens(value: unknown) {
  const number = Number(value)
  return Number.isFinite(number) && number > 0 ? number : 0
}

export function parseContextBreakdown(value: unknown): ContextBreakdown {
  const input = record(value)
  const known = new Map(
    (Array.isArray(input.categories) ? input.categories : []).map((item) => [
      record(item).id,
      record(item),
    ]),
  )
  const compactAt = Number(input.compactAtPercent)
  return {
    contextWindow: tokens(input.contextWindow),
    usedTokens: tokens(input.usedTokens),
    freeTokens: tokens(input.freeTokens),
    totalKnown: input.totalKnown === true,
    prefixKnown: input.prefixKnown === true,
    compactAtPercent:
      input.compactAtPercent != null && compactAt > 0 && compactAt <= 100 ? compactAt : null,
    // 顺序固定、颜色跟着分类走：某一项变成 0 或不见了，别的分类不换色。
    categories: CONTEXT_CATEGORY_IDS.map((id) => {
      const item = known.get(id) ?? {}
      return {
        id,
        tokens: tokens(item.tokens),
        count: item.count == null ? null : tokens(item.count),
        deferred: tokens(item.deferred),
        calls: tokens(item.calls),
        servers: (Array.isArray(item.servers) ? item.servers : []).map((server) => {
          const entry = record(server)
          return {
            name: typeof entry.name === 'string' ? entry.name : '',
            tools: tokens(entry.tools),
            calls: tokens(entry.calls),
            tokens: tokens(entry.tokens),
          }
        }),
      }
    }),
  }
}

// 占比：不到 1% 写成 <1%，不写成看起来是没有的 0%。
export function contextShareLabel(value: number, total: number) {
  if (!total || value <= 0) return '0%'
  const percent = (value / total) * 100
  return percent < 1 ? '<1%' : `${Math.round(percent)}%`
}
