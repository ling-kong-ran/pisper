export type SessionContextPreference = 'auto' | 'open' | 'closed'
export type SessionContextPresentation = 'aside' | 'sheet' | 'closed'

export type SessionContextRun = {
  sessionId: string
  streaming: boolean
  completed: boolean
}

// 只跟随当前会话的运行终态；打开旧会话或后台会话结束都不能抢占面板。
export function shouldRevealSessionContext(
  previous: SessionContextRun | null,
  current: SessionContextRun,
): boolean {
  return Boolean(
    current.sessionId &&
    previous?.sessionId === current.sessionId &&
    previous.streaming &&
    !current.streaming &&
    current.completed,
  )
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
