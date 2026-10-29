// 看板卡片的共享推导：权威缓存 + 实时覆盖层 + Runtime 能力 → 已排序的卡片列表（未筛选）。
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
  const capabilities = useBoardCapabilities()
  const sessions = query.data?.sessions
  // “今天”以最近一次快照时间为准：轮询期间每 3 秒推进，结果在同一份数据内保持稳定。
  const now = query.dataUpdatedAt
  const items = useMemo(
    () =>
      sessions
        ? buildBoardItems(
            sessions,
            { live, localRuns, runningSince, patches },
            { now, capabilities, doneWindow },
          )
        : [],
    [capabilities, doneWindow, live, localRuns, now, patches, runningSince, sessions],
  )
  return { query, items, capabilities }
}
