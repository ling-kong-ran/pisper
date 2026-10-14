// 实时会话同步辅助：判断当前状态里是否有仍在运行的 Agent，
// 供 UI 决定是否显示“正在运行”标记与保留关闭后的会话状态。
import type { SessionState } from '@/types/chat'

// 当前是否有活跃 agent（排队/启动/运行中），供 UI 决定运行标记与轮询策略。
export function hasActiveSessionAgents(state: Partial<SessionState> | undefined) {
  return Boolean(
    state?.agents?.some((agent) =>
      ['queued', 'starting', 'running'].includes(String(agent.status)),
    ),
  )
}

// 是否还需要轮询实时状态：本地已持有流（localStreamOwned）时不轮询；
// 否则在 恢复中/流式中/有待审批/有活跃 agent 时轮询补数据。
export function shouldPollLiveSession(
  state: Partial<SessionState> | undefined,
  { localStreamOwned = false }: { localStreamOwned?: boolean } = {},
) {
  if (!state || localStreamOwned) return false
  return Boolean(
    state.recovering || state.streaming || state.approvals?.length || hasActiveSessionAgents(state),
  )
}
