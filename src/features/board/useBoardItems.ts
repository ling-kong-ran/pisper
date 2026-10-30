// 看板卡片的共享推导：权威缓存 + 实时覆盖层（看板条目、审批详情、本地运行）+ Runtime 能力 → 已排序的卡片列表（未筛选）。
import { useMemo } from 'react'
import type { BoardCapabilities, BoardDoneWindow } from '@shared/session-board-status.mjs'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { buildBoardItems } from './board-model'
import { useBoardOverlay } from './board-store'
import { useSessionListQuery } from './useSessionListQuery'

export type BoardRuntimeCapabilities = Required<BoardCapabilities>

// askUser / sessionQueue 在第 6 期才由 Runtime 声明；没有声明时“等你回答”和“排队”永不出现。
export function useBoardCapabilities(): BoardRuntimeCapabilities {
  const features = useRuntimeCapabilitiesStore((state) => state.capabilities.features)
  return useMemo(() => {
    const flags: Partial<Record<string, boolean>> = features
    return { askUser: flags.askUser === true, sessionQueue: flags.sessionQueue === true }
  }, [features])
}

export function useBoardItems(doneWindow: BoardDoneWindow) {
  const query = useSessionListQuery()
  const live = useBoardOverlay((state) => state.live)
  const localRuns = useBoardOverlay((state) => state.localRuns)
  const runningSince = useBoardOverlay((state) => state.runningSince)
  const patches = useBoardOverlay((state) => state.patches)
  const pending = useBoardOverlay((state) => state.pending)
  // 只订阅条目与就绪标记：心跳只推进修订号，不让看板读者重算。
  const entries = useBoardOverlay((state) => state.feed.entries)
  const feedReady = useBoardOverlay((state) => state.feed.ready)
  // 分钟级时钟：事件流在线时目录可能很久不刷新，“今天”的判断跟着最近的数据推进。
  const feedMinute = useBoardOverlay((state) => Math.floor(state.feed.updatedAt / 60_000))
  const capabilities = useBoardCapabilities()
  const sessions = query.data?.sessions
  // “今天”以最近一次快照时间为准：轮询或事件流推进，结果在同一份数据内保持稳定。
  const now = Math.max(query.dataUpdatedAt, feedMinute * 60_000)
  const feed = useMemo(() => ({ ready: feedReady, entries }), [entries, feedReady])
  const items = useMemo(
    () =>
      sessions || feed.ready
        ? buildBoardItems(
            sessions ?? [],
            { live, localRuns, runningSince, patches, pending, feed },
            { now, capabilities, doneWindow },
          )
        : [],
    [
      capabilities,
      doneWindow,
      feed,
      live,
      localRuns,
      now,
      patches,
      pending,
      runningSince,
      sessions,
    ],
  )
  // 第 2 期条目先到、目录还在读取时，看板已经可以显示。
  const isLoading = query.isPending && !feed.ready
  return { query, items, capabilities, isLoading }
}
