// 会话列表合并：以服务端列表为准，保留本地乐观插入（尚未同步的）项，
// 避免新建会话后列表闪回。
import type { SessionSummary } from '@/types/chat'

export function mergeSessionLists(current: SessionSummary[], incoming: SessionSummary[]) {
  const incomingIds = new Set(incoming.map((session) => session.id))
  const optimistic = current.filter((session) => !incomingIds.has(session.id))
  return [...incoming, ...optimistic]
}

// 最近会话的工作目录：按列表顺序找第一个有 cwd 的会话，用于新会话继承。
export function recentSessionCwd(sessions: SessionSummary[]) {
  for (const session of sessions) {
    const cwd = typeof session.cwd === 'string' ? session.cwd.trim() : ''
    if (cwd) return cwd
  }
  return ''
}

// 从平铺会话列表移除一个 id。
export function removeTiledSession(ids: string[], sessionId: string) {
  return ids.filter((id) => id !== sessionId)
}

// 平铺会话开关：已含则移除，否则追加（保持顺序）。
export function toggleTiledSession(ids: string[], sessionId: string) {
  return ids.includes(sessionId) ? removeTiledSession(ids, sessionId) : [...ids, sessionId]
}
