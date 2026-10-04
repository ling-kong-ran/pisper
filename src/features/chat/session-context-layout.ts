import type { SessionFileChangeFile } from './chat-api'

export type SessionContextPreference = 'auto' | 'open' | 'closed'
export type SessionContextPresentation = 'aside' | 'sheet' | 'closed'

export type SessionContextRun = {
  sessionId: string
  streaming: boolean
  completed: boolean
  runStartedAt?: string | null
}

// 目录摘要可能先显示“运行中”，实时快照随后给出已完成的历史轮次。
// 只有前后都确认是同一轮运行，才把流结束视为本页观察到的完成事件。
export function didCompleteSessionContextRun(
  previous: SessionContextRun | null,
  current: SessionContextRun,
): boolean {
  return Boolean(
    current.sessionId &&
    previous?.sessionId === current.sessionId &&
    previous.streaming &&
    !previous.completed &&
    !current.streaming &&
    current.completed &&
    previous.runStartedAt &&
    Number.isFinite(Date.parse(previous.runStartedAt)) &&
    previous.runStartedAt === current.runStartedAt,
  )
}

// 文件快照覆盖整个会话，只展示本轮留下的有效改动，不能用历史文件数量决定自动打开。
export function shouldRevealSessionContext(
  previous: SessionContextRun | null,
  current: SessionContextRun,
  files: readonly SessionFileChangeFile[] = [],
): boolean {
  if (!didCompleteSessionContextRun(previous, current)) return false
  const startedAt = Date.parse(current.runStartedAt || '')
  if (!Number.isFinite(startedAt)) return false
  return files.some((file) => {
    const changedAt = Date.parse(file.changedAt)
    if (file.reverted || !Number.isFinite(changedAt) || changedAt < startedAt) return false
    return (
      file.added > 0 ||
      file.removed > 0 ||
      file.status === 'created' ||
      file.status === 'deleted' ||
      (!file.snapshot && file.pending)
    )
  })
}

export const SESSION_CONTEXT_DEFAULT_WIDTH = 360
export const SESSION_CONTEXT_MIN_WIDTH = 280
export const SESSION_CONTEXT_MAX_WIDTH = 720
export const SESSION_CHAT_MIN_WIDTH = 400

export function normalizeSessionContextWidth(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) return SESSION_CONTEXT_DEFAULT_WIDTH
  return Math.min(SESSION_CONTEXT_MAX_WIDTH, Math.max(SESSION_CONTEXT_MIN_WIDTH, Math.round(value)))
}

export function resolveSessionContextPresentation({
  availableWidth,
  mobileLayout,
  hasSession,
  preference,
}: {
  availableWidth: number
  mobileLayout: boolean
  hasSession: boolean
  preference: SessionContextPreference
}): SessionContextPresentation {
  if (!hasSession || preference === 'closed') return 'closed'
  const compact = mobileLayout || availableWidth < 800
  if (preference === 'auto' && compact) return 'closed'
  return compact ? 'sheet' : 'aside'
}
