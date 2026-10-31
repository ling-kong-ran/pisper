// ctx.sessions 的只读投影（规格 5.6、计划 §4.2）：会话目录 + 看板泳道 → {id,title,cwd,status,updatedAt,usage?}。
// 纯函数与一个小的外部 store：数据由宿主从看板数据层喂进来（sessions-source.ts），这里不导入看板，
// 便于测试，也让不声明 sessions.read 的组件根本不启动看板同步。项目按看板同一个工作区键比较。
import { boardWorkspaceKey } from '@shared/session-board-status.mjs'

export type ComponentSessionStatus = 'attention' | 'running' | 'queued' | 'done' | 'idle'
export type ComponentSessionSummary = {
  id: string
  title: string
  cwd: string
  status: ComponentSessionStatus
  updatedAt: string
  usage?: { totalTokens?: number }
}
export type ComponentSessionFilter = {
  cwd?: string
  status?: ComponentSessionStatus
  limit?: number
}

export const SESSION_LIST_DEFAULT_LIMIT = 50
export const SESSION_LIST_MAX_LIMIT = 100
const STATUSES: ReadonlySet<string> = new Set(['attention', 'running', 'queued', 'done', 'idle'])

export type CatalogSession = {
  id: string
  title: string
  cwd?: string
  modified?: string
  archived?: boolean
}
export type LaneSession = {
  id: string
  lane: 'attention' | 'running' | 'queued' | 'done'
  totalTokens?: number | null
}

/** 目录里的未归档会话按看板泳道标状态（不在看板上的是 idle），按更新时间倒序。 */
export function projectComponentSessions(
  catalog: readonly CatalogSession[],
  lanes: readonly LaneSession[],
): ComponentSessionSummary[] {
  const byId = new Map(lanes.map((item) => [item.id, item]))
  const result: ComponentSessionSummary[] = []
  for (const session of catalog) {
    if (session.archived) continue
    const lane = byId.get(session.id)
    const summary: ComponentSessionSummary = {
      id: session.id,
      title: session.title,
      cwd: session.cwd ?? '',
      status: lane?.lane ?? 'idle',
      updatedAt: session.modified ?? '',
    }
    if (typeof lane?.totalTokens === 'number' && Number.isFinite(lane.totalTokens))
      summary.usage = { totalTokens: lane.totalTokens }
    result.push(Object.freeze(summary))
  }
  return result.sort(
    (left, right) => (Date.parse(right.updatedAt) || 0) - (Date.parse(left.updatedAt) || 0),
  )
}

/** 组件传入的筛选按不可信输入处理：未知状态忽略，limit 夹到 1–100（缺省 50）。 */
export function normalizeSessionFilter(
  filter: unknown,
): Required<Pick<ComponentSessionFilter, 'limit'>> & Omit<ComponentSessionFilter, 'limit'> {
  const source =
    filter && typeof filter === 'object' && !Array.isArray(filter)
      ? (filter as Record<string, unknown>)
      : {}
  const limit =
    typeof source.limit === 'number' && Number.isFinite(source.limit)
      ? Math.min(SESSION_LIST_MAX_LIMIT, Math.max(1, Math.floor(source.limit)))
      : SESSION_LIST_DEFAULT_LIMIT
  return {
    limit,
    ...(typeof source.cwd === 'string' && source.cwd ? { cwd: source.cwd } : {}),
    ...(typeof source.status === 'string' && STATUSES.has(source.status)
      ? { status: source.status as ComponentSessionStatus }
      : {}),
  }
}

export function filterComponentSessions(
  sessions: readonly ComponentSessionSummary[],
  filter: unknown,
): ComponentSessionSummary[] {
  const { cwd, status, limit } = normalizeSessionFilter(filter)
  const key = cwd ? boardWorkspaceKey(cwd) : null
  const result: ComponentSessionSummary[] = []
  for (const session of sessions) {
    if (status && session.status !== status) continue
    if (key !== null && boardWorkspaceKey(session.cwd) !== key) continue
    result.push(session)
    if (result.length >= limit) break
  }
  return result
}

export type SessionFeed = {
  get(): readonly ComponentSessionSummary[]
  set(sessions: readonly ComponentSessionSummary[]): void
  subscribe(listener: (sessions: readonly ComponentSessionSummary[]) => void): () => void
}

/** ctx.sessions.subscribe 的数据源：宿主在渲染声明了 sessions.read 的组件时把最新投影写进来。 */
export function createSessionFeed(): SessionFeed {
  let current: readonly ComponentSessionSummary[] = []
  const listeners = new Set<(sessions: readonly ComponentSessionSummary[]) => void>()
  return {
    get: () => current,
    set(sessions) {
      if (
        sessions.length === current.length &&
        sessions.every((session, index) => session === current[index])
      )
        return
      current = sessions
      for (const listener of [...listeners]) listener(current)
    },
    subscribe(listener) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
  }
}
