// 会话列表合并：以服务端列表为准，保留本地乐观插入（尚未同步的）项，
// 避免新建会话后列表闪回。
import type { SessionSummary } from '@/types/chat'
import type { SessionOrganizationUpdate, SessionTitleUpdate } from './events'

export function mergeSessionLists(current: SessionSummary[], incoming: SessionSummary[]) {
  const incomingIds = new Set(incoming.map((session) => session.id))
  const optimistic = current.filter((session) => !incomingIds.has(session.id))
  return [...incoming, ...optimistic]
}

// 外部页面已完成重命名时，只替换目录中对应摘要，保留其它会话与实时状态。
export function applySessionTitleUpdate(
  sessions: SessionSummary[],
  { id, name }: SessionTitleUpdate,
) {
  if (!sessions.some((session) => session.id === id && session.name !== name)) return sessions
  return sessions.map((session) => (session.id === id ? { ...session, name } : session))
}

// 只覆盖请求发出后确认的标题变更；之后的新请求仍以服务端目录为准。
// 即使目录尚未加载，也先记录事件，避免初始化等待配置时丢掉重命名。
export function createSessionTitleReconciler() {
  let revision = 0
  const updates = new Map<string, { name: string; revision: number }>()
  return {
    getRevision: () => revision,
    record({ id, name }: SessionTitleUpdate) {
      updates.set(id, { name, revision: ++revision })
    },
    remove(ids: string[]) {
      for (const id of ids) updates.delete(id)
    },
    reconcile(sessions: SessionSummary[], requestRevision: number) {
      return sessions.map((session) => {
        const update = updates.get(session.id)
        return update && update.revision > requestRevision && update.name !== session.name
          ? { ...session, name: update.name }
          : session
      })
    },
  }
}

export function applySessionOrganizationUpdate(
  sessions: SessionSummary[],
  update: SessionOrganizationUpdate,
) {
  if (!sessions.some((session) => session.id === update.id)) return sessions
  return sessions.map((session) =>
    session.id === update.id
      ? {
          ...session,
          pinned: update.pinned,
          archived: update.archived,
          unread: update.unread,
          needsAttention: update.needsAttention,
          attentionReason: update.attentionReason,
        }
      : session,
  )
}

// 置顶只调整所在工作区/列表内的显示次序，不篡改 modified 的历史语义。
export function orderVisibleSessions<T extends SessionSummary>(sessions: T[]): T[] {
  return [...sessions].sort(
    (a, b) =>
      Number(Boolean(b.pinned)) - Number(Boolean(a.pinned)) ||
      (Date.parse(b.modified || '') || 0) - (Date.parse(a.modified || '') || 0),
  )
}

// 最近会话的工作目录：按列表顺序找第一个有 cwd 的会话，用于新会话继承。
export function recentSessionCwd(sessions: SessionSummary[]) {
  for (const session of sessions) {
    const cwd = typeof session.cwd === 'string' ? session.cwd.trim() : ''
    if (cwd) return cwd
  }
  return ''
}

// 根据路由模式决定是否允许把桌面会话目录带入新会话。
export function shouldInheritRecentSessionCwd(
  mobileApp: boolean,
  state?: { paired?: boolean; mode?: string | null } | null,
) {
  return !mobileApp || (state?.paired === true && state.mode === 'remote')
}

// 显式目录优先；未指定时按调用方策略选择最近目录或 Runtime 默认目录。
export function sessionCwdForCreate(
  cwd: string,
  sessions: SessionSummary[],
  inheritRecentCwd = true,
) {
  return cwd || (inheritRecentCwd ? recentSessionCwd(sessions) : '')
}

// 从平铺会话列表移除一个 id。
export function removeTiledSession(ids: string[], sessionId: string) {
  return ids.filter((id) => id !== sessionId)
}

// 平铺会话开关：已含则移除，否则追加（保持顺序）。
export function toggleTiledSession(ids: string[], sessionId: string) {
  return ids.includes(sessionId) ? removeTiledSession(ids, sessionId) : [...ids, sessionId]
}
