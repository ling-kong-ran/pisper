// 会话树摘要（规格 v2 第 4.5 节）：顶栏 ⑂ 的分支数与转录里的回合标记共用一份查询缓存。
// 整棵树可能上万个节点，缓存里只留分支数与“条目 → 标记名”，对话框打开时仍自己拉完整的树。
// 改标记、“从此处继续”、撤销等操作拿到 Runtime 返回的新树后立刻写回这份缓存（老问题 1、6：
// 标记在对话里看不出来、分支数更新滞后），不必等下一次会话树事件。
import { useEffect, useRef } from 'react'
import { useQuery } from '@tanstack/react-query'
import { queryClient } from '@/lib/startup-queries'
import { chatApi, type SessionTreeResponse } from './chat-api'

export type SessionTreeSummary = {
  branchCount: number
  // 条目 id → 标记名；只收有标记的条目。
  labels: Readonly<Record<string, string>>
}

const EMPTY_LABELS: Readonly<Record<string, string>> = Object.freeze({})

export function sessionTreeSummaryKey(sessionId: string) {
  return ['session-tree-summary', sessionId] as const
}

// 不可信输入：只认数字分支数与字符串 id / 标记；其余字段丢掉。
export function summarizeSessionTree(tree: unknown): SessionTreeSummary {
  const value = (tree && typeof tree === 'object' ? tree : {}) as Partial<SessionTreeResponse>
  const branchCount = Number(value.branchCount)
  const labels: Record<string, string> = {}
  for (const node of Array.isArray(value.nodes) ? value.nodes : []) {
    if (!node || typeof node.id !== 'string' || typeof node.label !== 'string') continue
    const label = node.label.trim()
    if (label) labels[node.id] = label.slice(0, 80)
  }
  return {
    branchCount: Number.isFinite(branchCount) && branchCount > 0 ? Math.floor(branchCount) : 0,
    labels: Object.keys(labels).length ? labels : EMPTY_LABELS,
  }
}

export async function fetchSessionTreeSummary(sessionId: string) {
  return summarizeSessionTree(await chatApi.getSessionTree(sessionId))
}

// 写回 Runtime 刚返回的整棵树（改标记、导航的响应都带树）；紧凑导航没有树时改为失效重读。
export function applySessionTreeResponse(sessionId: string, tree: unknown) {
  if (!sessionId) return
  const value = tree as Partial<SessionTreeResponse> | null
  if (value && Array.isArray(value.nodes)) {
    queryClient.setQueryData(sessionTreeSummaryKey(sessionId), summarizeSessionTree(value))
  } else {
    void invalidateSessionTreeSummary(sessionId)
  }
}

// 弹层打开时要当前标记：缓存里有就直接用，没有（顶栏还没读到）才读一次并放进缓存。
export function readSessionTreeSummary(sessionId: string) {
  return queryClient.fetchQuery({
    queryKey: sessionTreeSummaryKey(sessionId),
    queryFn: () => fetchSessionTreeSummary(sessionId),
    staleTime: Infinity,
  })
}

export function invalidateSessionTreeSummary(sessionId: string) {
  return queryClient.invalidateQueries({ queryKey: sessionTreeSummaryKey(sessionId) })
}

// 顶栏用：会话有对话且不在运行中时读取；运行结束、会话树事件（revision）到来时重读。
// 运行中不读：回合还没落盘，读到的是半截树，且没有用户操作会在运行中改变分支数。
export function useSessionTreeSummary(
  sessionId: string,
  { enabled, streaming, revision = 0 }: { enabled: boolean; streaming: boolean; revision?: number },
) {
  const active = enabled && !streaming && Boolean(sessionId)
  // 显式传入应用唯一的 queryClient：这份缓存与 Provider 无关，转录在 SSR 与冒烟夹具里也能渲染。
  const query = useQuery(
    {
      queryKey: sessionTreeSummaryKey(sessionId),
      queryFn: () => fetchSessionTreeSummary(sessionId),
      enabled: active,
      staleTime: Infinity,
    },
    queryClient,
  )
  // 首次挂载由查询自己读取；之后每次运行结束或 revision 变化才失效重读。
  const seen = useRef({ sessionId, streaming, revision })
  useEffect(() => {
    const previous = seen.current
    seen.current = { sessionId, streaming, revision }
    if (!active || previous.sessionId !== sessionId) return
    if (previous.streaming === streaming && previous.revision === revision) return
    void invalidateSessionTreeSummary(sessionId)
  }, [active, revision, sessionId, streaming])
  return query.data ?? null
}

// 转录里的一条回复：只订阅缓存，不自己发请求（顶栏负责读取），标记名变了才重渲染这一条。
export function useTurnLabel(sessionId: string, entryId: string | undefined) {
  const query = useQuery(
    {
      queryKey: sessionTreeSummaryKey(sessionId),
      queryFn: () => fetchSessionTreeSummary(sessionId),
      enabled: false,
      staleTime: Infinity,
      select: (summary: SessionTreeSummary) => (entryId ? summary.labels[entryId] || '' : ''),
    },
    queryClient,
  )
  return entryId ? query.data || '' : ''
}
