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
export const BOARD_CARD_STATUSES: readonly ['attention', 'running', 'queued', 'done', 'idle']
export const BOARD_CARD_OUTCOMES: readonly ['completed', 'failed', 'stopped']
export const COMMAND_GUARD_REASON_PREFIX: string

export type BoardLane = (typeof BOARD_LANES)[number]
export type BoardStatus = (typeof BOARD_STATUSES)[number]
export type ApprovalSeverity = (typeof APPROVAL_SEVERITIES)[number]
export type BoardDoneWindow = (typeof BOARD_DONE_WINDOWS)[number]
export type BoardCardStatus = (typeof BOARD_CARD_STATUSES)[number]
export type BoardCardOutcome = (typeof BOARD_CARD_OUTCOMES)[number]

// GET /api/sessions?view=board 与 /api/board/events 的 card 结构（第 2 期 Runtime，全部字段可选）。
// 客户端必须容忍未知字段与未知枚举值（例如 activity.stage、error.kind、changeStats.status 的新取值）。
export type BoardCardApproval = {
  id: string
  toolName: string
  // 预览：最多 512 个码点，已去掉控制字符；argsTruncated 为 true 时完整参数须从 /live 取。
  argsPreview: string
  argsTruncated?: boolean
  risk: string
  severity: ApprovalSeverity
  reason: string
  createdAt: string
}
export type BoardCardActivity =
  | { type: 'model'; stage: string }
  | { type: 'tool'; name: string; argsSummary: string; startedAt: string | null }
export type BoardCard = {
  boardStatus: BoardCardStatus
  runId?: string
  activity?: BoardCardActivity
  startedAt?: string
  lastActivityAt?: string
  finishedAt?: string
  outcome?: BoardCardOutcome
  error?: { message: string; kind: string }
  approvals?: BoardCardApproval[]
  queuedInputCount?: number
  contextPct?: number
  changeStats?: {
    added: number | null
    removed: number | null
    files: number | null
    status: string
  }
  lastAssistantPreview?: string
  tail?: string[]
  agents?: { running: number; total: number }
  usage?: { totalTokens: number }
}
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
export function boardWorkspaceKey(cwd: unknown): string
export function deriveApprovalSeverity(approval: unknown): ApprovalSeverity
export function approvalSeverity(approval: unknown): ApprovalSeverity
export function deriveBoardStatus(input: unknown, options: BoardStatusOptions): BoardStatusResult
export function compareBoardItems(a: BoardSortable, b: BoardSortable): number
export function sortBoardItems<T extends BoardSortable>(items: readonly T[]): T[]
