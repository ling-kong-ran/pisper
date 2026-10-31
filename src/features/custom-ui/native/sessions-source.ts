// ctx.sessions 的真实数据源：从看板数据层（会话目录 + 泳道）投影出只读摘要。
// 只有声明了 sessions.read 的组件才会调用这些 hook，所以不读会话的组件不会启动看板同步。
import { useEffect, useMemo } from 'react'
import { boardSessionTitle, useBoardSessions, useSessionListQuery } from '@/features/board/public'
import {
  createSessionFeed,
  projectComponentSessions,
  type ComponentSessionSummary,
} from './sessions'

// 全页共用一份订阅源：多个组件同时声明 sessions.read 时写入的是同一份投影。
export const componentSessionFeed = createSessionFeed()

export function useComponentSessionList(): readonly ComponentSessionSummary[] {
  const query = useSessionListQuery()
  // “今天完成”与全部代理页一致；更早完成的会话在投影里是 idle。
  const { all } = useBoardSessions({ doneWindow: 'today' })
  const catalog = query.data?.sessions
  return useMemo(
    () =>
      projectComponentSessions(
        (catalog ?? []).map((session) => ({
          id: session.id,
          title: boardSessionTitle(session),
          cwd: typeof session.cwd === 'string' ? session.cwd : '',
          modified: typeof session.modified === 'string' ? session.modified : '',
          archived: session.archived === true,
        })),
        all.map((item) => ({ id: item.id, lane: item.lane, totalTokens: item.totalTokens })),
      ),
    [all, catalog],
  )
}

/** 宿主在渲染声明了 sessions.read 的组件时挂一个：把最新投影写进 componentSessionFeed。 */
export function ComponentSessionFeed() {
  const sessions = useComponentSessionList()
  useEffect(() => {
    componentSessionFeed.set(sessions)
  }, [sessions])
  return null
}
