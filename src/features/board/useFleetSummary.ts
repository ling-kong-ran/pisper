// 舰队计量：运行中 / 排队 / 需要你的计数与等待最久的事项，供标题栏计量、⌥A 跳转与手机徽标使用。
// 不受看板筛选影响；只负责跳转，永远不批准任何审批。
import { useMemo } from 'react'
import { summarizeFleet, type FleetSummary } from './board-model'
import { useBoardItems, type BoardRuntimeCapabilities } from './useBoardItems'

export type FleetSummaryResult = FleetSummary & {
  // 排队计量只在 Runtime 声明 sessionQueue 时显示；标题栏据此决定是否渲染“◌ 排队”。
  capabilities: BoardRuntimeCapabilities
  isLoading: boolean
  error: unknown
}

export function useFleetSummary(): FleetSummaryResult {
  const { query, items, capabilities, isLoading } = useBoardItems('today')
  const summary = useMemo(() => summarizeFleet(items), [items])
  return useMemo(
    () => ({ ...summary, capabilities, isLoading, error: query.error ?? null }),
    [capabilities, isLoading, query.error, summary],
  )
}
