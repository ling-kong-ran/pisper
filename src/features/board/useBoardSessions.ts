// 看板主数据：带泳道推导、视图偏好筛选与固定排序的卡片，以及加载与错误状态。
import { useCallback, useMemo } from 'react'
import type { BoardDoneWindow, BoardLane } from '@shared/session-board-status.mjs'
import {
  boardProjects,
  filterBoardItems,
  groupBoardLanes,
  type BoardItem,
  type BoardProject,
} from './board-model'
import { refreshBoardFeed } from './board-store'
import { useBoardViewStore, type BoardViewPreferences } from './board-view-store'
import { useBoardItems, type BoardRuntimeCapabilities } from './useBoardItems'

export type BoardSessionsResult = {
  // 筛选前的全部卡片（按泳道规则排序）：全部代理页自己按分组规则推导（overview-model）。
  all: BoardItem[]
  // 已按项目与状态筛选、按泳道规则排序。
  items: BoardItem[]
  lanes: Record<BoardLane, BoardItem[]>
  // 筛选前的项目候选，供项目筛选与按项目分组使用。
  projects: BoardProject[]
  // 筛选前看板上的卡片数，用于区分“看板为空”和“筛选后为空”。
  total: number
  // 筛选前的需要你卡片：页头胶囊、逐个处理与确认层按全部事项计算，不受筛选影响。
  attentionAll: BoardItem[]
  // 目录里没有任何有内容的未归档会话（首次运行）；与“今天没有活动”区分文案和引导。
  catalogEmpty: boolean
  view: BoardViewPreferences
  capabilities: BoardRuntimeCapabilities
  // 首次加载尚无数据。
  isLoading: boolean
  isRefreshing: boolean
  // 最近一次刷新失败；有上一份快照时 items 仍可用。
  error: unknown
  refresh: () => Promise<void>
}

// doneWindow：桌面全部代理固定“今天完成”（规格 3.2），手机看板（A4 前）仍读偏好里的时间窗。
export function useBoardSessions({
  doneWindow: fixedWindow,
}: { doneWindow?: BoardDoneWindow } = {}): BoardSessionsResult {
  const groupBy = useBoardViewStore((state) => state.groupBy)
  const projectFilter = useBoardViewStore((state) => state.projects)
  const laneFilter = useBoardViewStore((state) => state.lanes)
  const collapsedLanes = useBoardViewStore((state) => state.collapsedLanes)
  const preferredWindow = useBoardViewStore((state) => state.doneWindow)
  const doneWindow = fixedWindow ?? preferredWindow
  const { query, items: all, capabilities, isLoading } = useBoardItems(doneWindow)
  const items = useMemo(
    () => filterBoardItems(all, { lanes: laneFilter, projects: projectFilter }),
    [all, laneFilter, projectFilter],
  )
  const lanes = useMemo(() => groupBoardLanes(items), [items])
  const projects = useMemo(() => boardProjects(all), [all])
  const attentionAll = useMemo(() => all.filter((item) => item.lane === 'attention'), [all])
  const sessions = query.data?.sessions
  // 空草稿会话（没有任何消息）不算历史：它们本来就不上看板。
  // 目录还没读到（看板条目先到）时不下“首次运行”的结论。
  const catalogEmpty = useMemo(
    () =>
      Boolean(sessions) &&
      !(sessions ?? []).some(
        (session) =>
          session.archived !== true &&
          (typeof session.messageCount !== 'number' || session.messageCount > 0),
      ),
    [sessions],
  )
  const view = useMemo(
    () => ({ groupBy, projects: projectFilter, lanes: laneFilter, collapsedLanes, doneWindow }),
    [collapsedLanes, doneWindow, groupBy, laneFilter, projectFilter],
  )
  const refresh = useCallback(async () => {
    await refreshBoardFeed()
  }, [])
  return {
    all,
    items,
    lanes,
    projects,
    total: all.length,
    attentionAll,
    catalogEmpty,
    view,
    capabilities,
    isLoading,
    isRefreshing: query.isFetching && !query.isPending,
    error: query.error ?? null,
    refresh,
  }
}
