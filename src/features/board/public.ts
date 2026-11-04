// @public 看板领域对应用壳、侧栏、聊天页提供的数据入口；不导出页面组件，也没有导入副作用。
export { useBoardActions } from './useBoardActions'
export { useBoardSessions, type BoardSessionsResult } from './useBoardSessions'
export { useSidebarModel, type SidebarModelResult } from './useSidebarModel'
export { useFleetSystemInfo, type FleetSystemInfo } from './useFleetSystemInfo'
export { useSessionListQuery } from './useSessionListQuery'
// 新任务草稿（'/board/new'）的看板一侧：项目芯片、派发偏好、“排在…之后”与第一条消息前的建会话。
export { useNewTaskDraftChrome } from './new-task-draft'
export { BOARD_SESSIONS_QUERY_KEY } from './board-store'
export {
  parseComponentChange,
  subscribeComponentChanges,
  type ComponentChange,
  type ComponentChangeListener,
  type ComponentChangeReason,
} from './component-changes'
export { boardSessionTitle } from './board-model'
export type { BoardSessionsData, BoardActionKind } from './board-store'
export type { BoardActionResult } from './board-actions'
export type { BoardActivity, BoardApproval } from './board-api'
export type { BoardItem, BoardProject } from './board-model'
export type {
  ApprovalSeverity,
  BoardDoneWindow,
  BoardLane,
  BoardStatus,
} from '@shared/session-board-status.mjs'
export type { SessionAlert, SessionAlertKind } from './session-alerts'
