// @public 侧栏等组合边界可使用的工作区会话规则；不依赖页面或 Runtime。
// 展示列表可以搜索和截断，项目操作始终以完整会话目录为准。
export type WorkspaceSession = { id: string; cwd?: string }

export type WorkspaceSessionGroup<T extends WorkspaceSession> = {
  key: string
  cwd: string
  sessions: T[]
}

// NUL 不能出现在合法工作目录中，避免与名为 __no_workspace__ 的相对目录碰撞。
export const NO_WORKSPACE_KEY = '\0'

export function workspaceKey(cwd = '') {
  const path = cwd.trim().replace(/\\/g, '/')
  // 根目录也是可选项目，不能在移除尾斜杠时与“无工作区”合并。
  const normalized = /^\/+$/u.test(path) ? '/' : path.replace(/\/+$/, '')
  return /^[A-Za-z]:(?:\/|$)/.test(normalized) ? normalized.toLowerCase() : normalized
}

export function sessionWorkspaceKey(session: WorkspaceSession) {
  return workspaceKey(session.cwd) || NO_WORKSPACE_KEY
}

export function groupSessionsByWorkspace<T extends WorkspaceSession>(sessions: readonly T[]) {
  const groups = new Map<string, WorkspaceSessionGroup<T>>()
  for (const session of sessions) {
    const key = sessionWorkspaceKey(session)
    const group = groups.get(key) || { key, cwd: session.cwd || '', sessions: [] }
    group.sessions.push(session)
    groups.set(key, group)
  }
  return [...groups.values()]
}

// 最近条数限制只裁剪会话行；每个项目保留首条会话，避免新项目被置顶会话挤掉。
export function recentWorkspaceGroups<T extends WorkspaceSession>(
  sessions: readonly T[],
  limit: number,
  activeSessionId = '',
) {
  const recentIds = new Set(sessions.slice(0, limit).map((session) => session.id))
  return groupSessionsByWorkspace(sessions).map((group) => ({
    ...group,
    sessions: group.sessions.filter(
      (session, index) =>
        index === 0 || recentIds.has(session.id) || session.id === activeSessionId,
    ),
  }))
}

export function normalizeWorkspaceOrder(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  return [
    ...new Set(value.filter((key): key is string => typeof key === 'string').map(workspaceKey)),
  ].filter((key) => key === NO_WORKSPACE_KEY || (key.length > 0 && !key.includes('\0')))
}

// 搜索、归档和最近列表截断不代表目录已删除；保留旧位置，只在末尾记录新目录。
export function reconcileWorkspaceOrder(order: readonly string[], keys: readonly string[]) {
  return normalizeWorkspaceOrder([...order, ...keys])
}

export function orderWorkspaceGroups<T extends WorkspaceSession>(
  groups: readonly WorkspaceSessionGroup<T>[],
  order: readonly string[],
) {
  const positions = new Map(order.map((key, index) => [key, index]))
  // 首次发现的目录在保存偏好前也先放在末尾，避免 effect 写入前闪动。
  return [...groups].sort(
    (left, right) =>
      (positions.get(left.key) ?? order.length) - (positions.get(right.key) ?? order.length),
  )
}

export function sessionsInWorkspace<T extends WorkspaceSession>(
  sessions: readonly T[],
  key: string,
) {
  return sessions.filter((session) => sessionWorkspaceKey(session) === key)
}

export function replacementActiveSessionId(
  currentId: string,
  remaining: readonly WorkspaceSession[],
  deletedIds: ReadonlySet<string>,
  directoryVerified: boolean,
): string | null {
  if (!currentId) return null
  if (
    !deletedIds.has(currentId) &&
    (!directoryVerified || remaining.some((s) => s.id === currentId))
  ) {
    return null
  }
  return remaining[0]?.id || ''
}

// 单个删除失败后停止后续请求，返回已经确定成功的 id，让调用方同步活动会话和准确报数。
export async function deleteSessionsSequentially(
  ids: readonly string[],
  remove: (id: string) => Promise<unknown>,
) {
  const deletedIds: string[] = []
  for (const id of ids) {
    try {
      await remove(id)
      deletedIds.push(id)
    } catch (error) {
      return { deletedIds, failedId: id, error }
    }
  }
  return { deletedIds, failedId: null, error: null }
}
