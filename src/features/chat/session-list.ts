import type { SessionSummary } from '@/types/chat'

export function mergeSessionLists(current: SessionSummary[], incoming: SessionSummary[]) {
  const incomingIds = new Set(incoming.map((session) => session.id))
  const optimistic = current.filter((session) => !incomingIds.has(session.id))
  return [...incoming, ...optimistic]
}

export function recentSessionCwd(sessions: SessionSummary[]) {
  for (const session of sessions) {
    const cwd = typeof session.cwd === 'string' ? session.cwd.trim() : ''
    if (cwd) return cwd
  }
  return ''
}

export function removeTiledSession(ids: string[], sessionId: string) {
  return ids.filter((id) => id !== sessionId)
}

export function toggleTiledSession(ids: string[], sessionId: string) {
  return ids.includes(sessionId) ? removeTiledSession(ids, sessionId) : [...ids, sessionId]
}
