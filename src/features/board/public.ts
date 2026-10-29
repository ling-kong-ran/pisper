// @public 看板领域对应用壳、侧栏、聊天页提供的数据入口；不导出页面组件，也没有导入副作用。
export { useBoardActions } from './useBoardActions'
export { useBoardSessions, type BoardSessionsResult } from './useBoardSessions'
export { useFleetSummary, type FleetSummaryResult } from './useFleetSummary'
export { useFleetSystemInfo, type FleetSystemInfo } from './useFleetSystemInfo'
export { useSessionListQuery } from './useSessionListQuery'
export {
  BOARD_NEW_TASK_EVENT,
  requestBoardNewTask,
  useBoardNewTaskHandler,
  type BoardNewTaskRequest,
  type BoardNewTaskSource,
} from './new-task'
export { BOARD_SESSIONS_QUERY_KEY } from './board-store'
export { boardSessionTitle } from './board-model'
export type { BoardSessionsData, BoardActionKind } from './board-store'
export type { BoardActionResult } from './board-actions'
export type { BoardActivity, BoardApproval } from './board-api'
export type { BoardItem, BoardProject, FleetAttentionItem, FleetSummary } from './board-model'
export type {
  ApprovalSeverity,
  BoardDoneWindow,
  BoardLane,
  BoardStatus,
} from '@shared/session-board-status.mjs'
