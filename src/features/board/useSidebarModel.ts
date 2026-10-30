// 侧栏分组数据（规格 v2 第 2.1、2.2 节）：读看板 store（与全部代理、手机标签栏同一个所有者），
// 经 sidebar-model 推导分组、计数与 ⌥A / ⌥↑⌥↓ 的顺序。分组按分钟级时钟重算（“今天完成”的边界），
// 行内的等待时长与运行计时另用每秒时钟，避免每秒重排整组。
import { useMemo } from 'react'
import { buildSidebarModel, type SidebarModel } from './sidebar-model'
import { useBoardItems, type BoardRuntimeCapabilities } from './useBoardItems'
import { useNow } from './useNow'

export type SidebarModelResult = {
  model: SidebarModel
  capabilities: BoardRuntimeCapabilities
  // 首次加载尚无数据：侧栏画骨架行，不把“还不知道”说成“没有代理”。
  isLoading: boolean
  // 最近一次刷新失败；有上一份快照时分组照常显示。
  error: unknown
  refetch: () => void
}

export function useSidebarModel(): SidebarModelResult {
  const { query, items, capabilities, isLoading } = useBoardItems('today')
  // 分钟级：只为“今天”的边界推进；运行中与需要你的计时由行自己的秒级时钟负责。
  const minute = Math.floor(useNow(true) / 60_000)
  const model = useMemo(
    () => buildSidebarModel(items, { now: minute * 60_000, capabilities }),
    [capabilities, items, minute],
  )
  const refetch = query.refetch
  return useMemo(
    () => ({
      model,
      capabilities,
      isLoading,
      error: query.error ?? null,
      refetch: () => void refetch(),
    }),
    [capabilities, isLoading, model, query.error, refetch],
  )
}
