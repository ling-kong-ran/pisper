import type { SessionSummary } from '@/types/chat'
import { orderVisibleSessions } from './session-list'

export const HISTORY_BATCH_SIZE = 50
export type HistoryView = 'active' | 'archived'
// 历史页分组：置顶会话保持在最前，其余按本地日历日分为今天、近 7 天与更早。
export type HistoryGroupKey = 'pinned' | 'today' | 'week' | 'earlier'
export type HistoryGroup = { key: HistoryGroupKey; count: number; items: SessionSummary[] }

const DAY_MS = 86_400_000

export function canSplitHistorySessions(compactDock: boolean, mobileApp: boolean): boolean {
  return !compactDock && !mobileApp
}

export function historyGroupKey(session: SessionSummary, now = Date.now()): HistoryGroupKey {
  if (session.pinned) return 'pinned'
  const modified = Date.parse(session.modified || '')
  if (!Number.isFinite(modified)) return 'earlier'
  const startOfToday = new Date(now)
  startOfToday.setHours(0, 0, 0, 0)
  if (modified >= startOfToday.getTime()) return 'today'
  // 近 7 天含今天，按日历日边界计算，避免同一天的会话因时刻不同落入两组。
  if (modified >= startOfToday.getTime() - 6 * DAY_MS) return 'week'
  return 'earlier'
}

// 计数覆盖完整筛选结果，分组内只放入已挂载的前 shown 条；尚未加载的分组不出现。
export function groupHistorySessions(
  ordered: SessionSummary[],
  shown: number,
  now = Date.now(),
): HistoryGroup[] {
  const groups = new Map<HistoryGroupKey, HistoryGroup>()
  ordered.forEach((session, index) => {
    const key = historyGroupKey(session, now)
    const group = groups.get(key) ?? { key, count: 0, items: [] }
    group.count += 1
    if (index < shown) group.items.push(session)
    groups.set(key, group)
  })
  return [...groups.values()].filter((group) => group.items.length > 0)
}

export function selectHistorySessions(
  sessions: SessionSummary[],
  query: string,
  limit: number,
  view: HistoryView = 'active',
  now = Date.now(),
): { total: number; items: SessionSummary[]; groups: HistoryGroup[] } {
  const needle = query.trim().toLowerCase()
  const filtered = needle
    ? sessions.filter((session) =>
        `${session.name || ''} ${session.firstMessage || ''} ${session.cwd || ''} ${session.model || ''}`
          .toLowerCase()
          .includes(needle),
      )
    : sessions.filter((session) => Boolean(session.archived) === (view === 'archived'))
  const ordered = orderVisibleSessions(filtered)
  const items = ordered.slice(0, Math.max(0, limit))
  return {
    total: filtered.length,
    items,
    groups: groupHistorySessions(ordered, items.length, now),
  }
}
