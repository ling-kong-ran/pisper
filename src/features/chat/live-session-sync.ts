import type { SessionState } from '@/types/chat'

export function hasActiveSessionAgents(state: Partial<SessionState> | undefined) {
  return Boolean(
    state?.agents?.some((agent) => ['queued', 'starting', 'running'].includes(String(agent.status))),
  )
}

export function shouldPollLiveSession(
  state: Partial<SessionState> | undefined,
  { localStreamOwned = false }: { localStreamOwned?: boolean } = {},
) {
  if (!state || localStreamOwned) return false
  return Boolean(state.recovering || state.approvals?.length || hasActiveSessionAgents(state))
}
