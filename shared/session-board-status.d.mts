export const BOARD_LANES: readonly ['attention', 'running', 'queued', 'done']
export const BOARD_STATUSES: readonly [
  'approval',
  'question',
  'failed',
  'running',
  'queued',
  'done',
  'stopped',
]
export const APPROVAL_SEVERITIES: readonly ['standard', 'critical']
export const BOARD_DONE_WINDOWS: readonly ['today', '7d']
export const COMMAND_GUARD_REASON_PREFIX: string

export type BoardLane = (typeof BOARD_LANES)[number]
export type BoardStatus = (typeof BOARD_STATUSES)[number]
export type ApprovalSeverity = (typeof APPROVAL_SEVERITIES)[number]
export type BoardDoneWindow = (typeof BOARD_DONE_WINDOWS)[number]
export type BoardCapabilities = { askUser?: boolean; sessionQueue?: boolean }
export type BoardStatusOptions = {
  now: number | string | Date
  timeZoneOffsetMinutes?: number
  capabilities?: BoardCapabilities
  doneWindow?: BoardDoneWindow
}
export type BoardStatusResult = {
  lane: BoardLane | null
  status: BoardStatus
  handled: boolean
  since: number | null
}
export type BoardSortable = {
  id: string
  lane: BoardLane | null
  since?: number | null
  pinned?: boolean
  queuePosition?: number | null
}

export function boardTimestamp(value: unknown): number | null
export function isWithinDoneWindow(
  value: unknown,
  options: {
    now: number | string | Date
    timeZoneOffsetMinutes?: number
    doneWindow?: BoardDoneWindow
  },
): boolean
export function isBoardLane(value: unknown): value is BoardLane
export function isBoardStatus(value: unknown): value is BoardStatus
export function approvalSeverity(approval: unknown): ApprovalSeverity
export function deriveBoardStatus(input: unknown, options: BoardStatusOptions): BoardStatusResult
export function compareBoardItems(a: BoardSortable, b: BoardSortable): number
export function sortBoardItems<T extends BoardSortable>(items: readonly T[]): T[]
