// 看板视图偏好：分组方式、项目与状态筛选、折叠的泳道、已完成时间窗。
// 各泳道内的排序是固定规则，不属于偏好。
import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import {
  BOARD_DONE_WINDOWS,
  isBoardLane,
  type BoardDoneWindow,
  type BoardLane,
} from '@shared/session-board-status.mjs'
import {
  boardPreferenceStorage,
  persistedRecord,
  persistedStrings,
} from './board-preference-storage'

export const BOARD_VIEW_PREFERENCE_KEY = 'pisper-board-view'
export const BOARD_GROUPINGS = ['lane', 'project'] as const
export type BoardGrouping = (typeof BOARD_GROUPINGS)[number]

export type BoardViewPreferences = {
  groupBy: BoardGrouping
  // 项目筛选保存工作区键（与 workspace-order-store 一致）；空数组表示全部项目。
  projects: string[]
  // 状态筛选按泳道：需要你 / 运行中 / 排队 / 已完成；空数组表示全部。
  lanes: BoardLane[]
  collapsedLanes: BoardLane[]
  doneWindow: BoardDoneWindow
}

type BoardViewState = BoardViewPreferences & {
  // 仪表区收起后只留一行 44px 的启动台；与视图偏好一起保存在 pisper-board-view。
  deckCollapsed: boolean
  setDeckCollapsed: (collapsed: boolean) => void
  setGroupBy: (groupBy: BoardGrouping) => void
  setProjectFilter: (projects: readonly string[]) => void
  toggleProjectFilter: (project: string) => void
  setLaneFilter: (lanes: readonly BoardLane[]) => void
  toggleLaneFilter: (lane: BoardLane) => void
  setLaneCollapsed: (lane: BoardLane, collapsed: boolean) => void
  setDoneWindow: (doneWindow: BoardDoneWindow) => void
  clearFilters: () => void
}

export const DEFAULT_BOARD_VIEW: BoardViewPreferences = {
  groupBy: 'lane',
  projects: [],
  lanes: [],
  collapsedLanes: [],
  doneWindow: 'today',
}

function lanes(value: unknown): BoardLane[] {
  return persistedStrings(value, { limit: 4, maxLength: 16 }).filter(isBoardLane)
}

export function normalizeBoardView(value: unknown): BoardViewPreferences {
  const source = persistedRecord(value)
  return {
    groupBy: BOARD_GROUPINGS.includes(source.groupBy as BoardGrouping)
      ? (source.groupBy as BoardGrouping)
      : DEFAULT_BOARD_VIEW.groupBy,
    projects: persistedStrings(source.projects),
    lanes: lanes(source.lanes),
    collapsedLanes: lanes(source.collapsedLanes),
    doneWindow: BOARD_DONE_WINDOWS.includes(source.doneWindow as BoardDoneWindow)
      ? (source.doneWindow as BoardDoneWindow)
      : DEFAULT_BOARD_VIEW.doneWindow,
  }
}

function toggled<T>(items: readonly T[], item: T) {
  return items.includes(item) ? items.filter((value) => value !== item) : [...items, item]
}

export const useBoardViewStore = create<BoardViewState>()(
  persist(
    (set) => ({
      ...DEFAULT_BOARD_VIEW,
      deckCollapsed: false,
      setDeckCollapsed: (deckCollapsed) => set({ deckCollapsed: deckCollapsed === true }),
      setGroupBy: (groupBy) => set({ groupBy: normalizeBoardView({ groupBy }).groupBy }),
      setProjectFilter: (projects) => set({ projects: persistedStrings([...projects]) }),
      toggleProjectFilter: (project) =>
        set((state) => ({ projects: persistedStrings(toggled(state.projects, project)) })),
      setLaneFilter: (value) => set({ lanes: lanes([...value]) }),
      toggleLaneFilter: (lane) => set((state) => ({ lanes: lanes(toggled(state.lanes, lane)) })),
      setLaneCollapsed: (lane, collapsed) =>
        set((state) => ({
          collapsedLanes: lanes(
            collapsed
              ? [...state.collapsedLanes, lane]
              : state.collapsedLanes.filter((item) => item !== lane),
          ),
        })),
      setDoneWindow: (doneWindow) =>
        set({ doneWindow: normalizeBoardView({ doneWindow }).doneWindow }),
      clearFilters: () => set({ projects: [], lanes: [] }),
    }),
    {
      name: BOARD_VIEW_PREFERENCE_KEY,
      storage: boardPreferenceStorage<BoardViewPreferences & { deckCollapsed: boolean }>(),
      version: 1,
      partialize: ({ groupBy, projects, lanes, collapsedLanes, doneWindow, deckCollapsed }) => ({
        groupBy,
        projects,
        lanes,
        collapsedLanes,
        doneWindow,
        deckCollapsed,
      }),
      // 持久化内容来自磁盘或其他端口，按不可信数据重新校验后再合并。
      merge: (persisted, current) => ({
        ...current,
        ...normalizeBoardView(persisted),
        deckCollapsed: persistedRecord(persisted).deckCollapsed === true,
      }),
    },
  ),
)
