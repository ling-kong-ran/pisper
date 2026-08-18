// 实时会话同步辅助：判断当前状态里是否有仍在运行的 Agent，
// 供 UI 决定是否显示“正在运行”标记与保留关闭后的会话状态。
import type { SessionState } from '@/types/chat'

export function hasActiveSessionAgents(state: Partial<SessionState> | undefined) {
  return Boolean(
    state?.agents?.some((agent) =>
      ['queued', 'starting', 'running'].includes(String(agent.status)),
    ),
  )
}

export function shouldPollLiveSession(
  state: Partial<SessionState> | undefined,
  { localStreamOwned = false }: { localStreamOwned?: boolean } = {},
) {
  if (!state || localStreamOwned) return false
  return Boolean(
    state.recovering || state.streaming || state.approvals?.length || hasActiveSessionAgents(state),
  )
}
