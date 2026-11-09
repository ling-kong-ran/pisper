import type { SessionSummary } from '@/types/chat'
import { orderVisibleSessions } from './session-list'
import { groupSessionsByWorkspace, sessionWorkspaceKey } from './session-workspaces'

export const HISTORY_BATCH_SIZE = 50
export type HistoryView = 'active' | 'archived'
// 历史页分组：置顶会话保持在最前，其余按本地日历日分为今天、近 7 天与更早。
export type HistoryGroupKey = 'pinned' | 'today' | 'week' | 'earlier'
export type HistoryGroup = { key: HistoryGroupKey; count: number; items: SessionSummary[] }
// 没选项目时按项目归类：每个项目先显示最近几条，其余从“查看全部”进入这个项目的筛选。
export const HISTORY_PROJECT_PREVIEW = 5
export type HistoryProjectGroup = {
  key: string
  cwd: string
  count: number
  items: SessionSummary[]
}

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

// 搜索覆盖未归档与已归档两边；project 是 sessionWorkspaceKey，空串表示全部项目。
function filterHistorySessions(
  sessions: SessionSummary[],
  query: string,
  view: HistoryView,
  project: string,
) {
  const needle = query.trim().toLowerCase()
  return sessions.filter(
    (session) =>
      (!project || sessionWorkspaceKey(session) === project) &&
      (needle
        ? `${session.name || ''} ${session.firstMessage || ''} ${session.lastMessage?.text || ''} ${session.cwd || ''} ${session.model || ''}`
            .toLowerCase()
            .includes(needle)
        : Boolean(session.archived) === (view === 'archived')),
  )
}

export function selectHistorySessions(
  sessions: SessionSummary[],
  query: string,
  limit: number,
  view: HistoryView = 'active',
  { project = '', now = Date.now() }: { project?: string; now?: number } = {},
): { total: number; items: SessionSummary[]; groups: HistoryGroup[] } {
  const filtered = filterHistorySessions(sessions, query, view, project)
  const ordered = orderVisibleSessions(filtered)
  const items = ordered.slice(0, Math.max(0, limit))
  return {
    total: filtered.length,
    items,
    groups: groupHistorySessions(ordered, items.length, now),
  }
}

// 按项目归类：最近有更新的项目在前（置顶只在项目内部靠前，不把整个项目顶上去）；
// count 是该项目在筛选结果里的会话数，items 只取前 preview 条。
export function selectHistoryProjects(
  sessions: SessionSummary[],
  query: string,
  view: HistoryView = 'active',
  preview = HISTORY_PROJECT_PREVIEW,
): { total: number; groups: HistoryProjectGroup[] } {
  const filtered = filterHistorySessions(sessions, query, view, '')
  const latest = (items: SessionSummary[]) =>
    Math.max(0, ...items.map((session) => Date.parse(session.modified || '') || 0))
  const groups = groupSessionsByWorkspace(orderVisibleSessions(filtered))
    .map((group) => ({ group, latest: latest(group.sessions) }))
    .sort((left, right) => right.latest - left.latest)
    .map(({ group }) => ({
      key: group.key,
      cwd: group.cwd,
      count: group.sessions.length,
      items: group.sessions.slice(0, Math.max(0, preview)),
    }))
  return { total: filtered.length, groups }
}
