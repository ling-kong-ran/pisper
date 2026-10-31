// 会话打开请求（写进 localStorage 的 pisper-session-open-request，跨页面 / 重启）：会话 id、打开方式与可选的消息定位。
// 第 3 期起分屏不再挂载，打开方式只剩 open 生效；left / right / above / below 仍按旧格式解析，
// 旧版本留下的请求照常打开会话（当作 open），不因字段值变化被丢弃。
export const SESSION_OPEN_DISPOSITIONS = Object.freeze([
  'open',
  'left',
  'right',
  'above',
  'below',
] as const)
export type SessionOpenDisposition = (typeof SESSION_OPEN_DISPOSITIONS)[number]

export type SessionOpenRequest = {
  sessionId: string
  disposition: SessionOpenDisposition
  targetEntryId?: string
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function createSessionOpenRequest(
  sessionId: string,
  disposition: string = 'open',
  targetEntryId = '',
): SessionOpenRequest | null {
  if (!sessionId || !SESSION_OPEN_DISPOSITIONS.includes(disposition as SessionOpenDisposition))
    return null
  return {
    sessionId,
    disposition: disposition as SessionOpenDisposition,
    ...(targetEntryId ? { targetEntryId } : {}),
  }
}

// 解析会话打开请求（容忍缺字段，默认 open / 空 target）。
export function parseSessionOpenRequest(raw: unknown): SessionOpenRequest | null {
  if (!raw) return null
  try {
    const value: unknown = typeof raw === 'string' ? JSON.parse(raw) : raw
    if (!isRecord(value)) return null
    const sessionId = typeof value.sessionId === 'string' ? value.sessionId : ''
    const disposition = typeof value.disposition === 'string' ? value.disposition : 'open'
    const targetEntryId = typeof value.targetEntryId === 'string' ? value.targetEntryId : ''
    return createSessionOpenRequest(sessionId, disposition, targetEntryId)
  } catch {
    return null
  }
}
