import type { SessionSummary } from '@/types/chat'
import { orderVisibleSessions } from './session-list'

export const HISTORY_BATCH_SIZE = 50
export type HistoryView = 'active' | 'archived'

export function canSplitHistorySessions(compactDock: boolean, mobileApp: boolean): boolean {
  return !compactDock && !mobileApp
}

export function selectHistorySessions(
  sessions: SessionSummary[],
  query: string,
  limit: number,
  view: HistoryView = 'active',
): { total: number; items: SessionSummary[] } {
  const needle = query.trim().toLowerCase()
  const filtered = needle
    ? sessions.filter((session) =>
        `${session.name || ''} ${session.firstMessage || ''} ${session.cwd || ''} ${session.model || ''}`
          .toLowerCase()
          .includes(needle),
      )
    : sessions.filter((session) => Boolean(session.archived) === (view === 'archived'))
  return {
    total: filtered.length,
    items: orderVisibleSessions(filtered).slice(0, Math.max(0, limit)),
  }
}
