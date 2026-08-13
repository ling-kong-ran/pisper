import { planFromPayload, planFromPayloadOr } from '@/lib/plan-protocol'
import type { ChatMessage, EntityRecord, Plan, SessionState, SessionSummary } from '@/types/chat'

export const DEFAULT_SESSION_STATE: SessionState = Object.freeze({
  messages: [],
  tools: [],
  approvals: [],
  agents: [],
  currentActivity: null,
  activityFeed: [],
  thinkingText: '',
  queuedInputs: [],
  hadQueuedInput: false,
  plan: null,
  executionMode: null,
  contextUsage: null,
  sessionUsage: null,
  promptCache: null,
  compaction: null,
  streaming: false,
  error: '',
  loaded: false,
  messageStart: null,
  hasOlder: false,
  olderCursor: null,
})

export function sessionStateChanged(previous?: SessionState, next?: SessionState) {
  if (previous === next) return false
  if (!previous || !next) return true
  const keys = new Set([...Object.keys(previous), ...Object.keys(next)])
  for (const key of keys) {
    if (previous[key] !== next[key]) return true
  }
  return false
}

export type SessionStateUpdate = Partial<SessionState> | ((current: SessionState) => SessionState)

export function applySessionUpdate(
  previous: SessionState | undefined,
  update: SessionStateUpdate,
): SessionState {
  const base = previous || DEFAULT_SESSION_STATE
  const next = typeof update === 'function' ? update(base) : { ...base, ...update }
  return sessionStateChanged(base, next) ? next : base
}

export function resolveQueuedInputs(
  current: EntityRecord[] | undefined,
  incoming: EntityRecord[] | null | undefined,
) {
  if (incoming === undefined) return current || []
  return Array.isArray(incoming) ? incoming : []
}

export function insertInteractiveUserMessage(
  messages: ChatMessage[] = [],
  message: ChatMessage,
): ChatMessage[] {
  const current = Array.isArray(messages) ? messages : []
  let activeAgentIndex = -1
  for (let index = current.length - 1; index >= 0; index -= 1) {
    const item = current[index]
    if (item?.role === 'agent') {
      activeAgentIndex = index
      break
    }
  }
  if (activeAgentIndex < 0) return [...current, message]
  return [...current.slice(0, activeAgentIndex), message, ...current.slice(activeAgentIndex)]
}

export function shouldRetainClosedSessionState(state: Partial<SessionState> | undefined) {
  return Boolean(
    state?.streaming ||
    state?.recovering ||
    state?.agents?.some((agent) => ['queued', 'starting', 'running'].includes(agent.status)),
  )
}

export function hasRunActivity(activity: EntityRecord | null | undefined) {
  return Boolean(
    activity &&
    (activity.streaming ||
      String(activity.thinkingText || '').trim() ||
      activity.tools?.length ||
      activity.currentActivity?.type === 'agent' ||
      activity.activityFeed?.some((item: EntityRecord) => item.type === 'agent')),
  )
}

export function resolveMessageRunActivity(
  message: ChatMessage,
  isLatestAgent: boolean,
  liveActivity: EntityRecord | null | undefined,
) {
  if (isLatestAgent && hasRunActivity(liveActivity)) return liveActivity
  return hasRunActivity(message.runActivity) ? message.runActivity : null
}

/**
 * Prefer live session state once a session has opened. Explicit null means
 * “cleared” and must not fall back to a stale listSessions snapshot.
 */
export function resolveSessionPlan(
  state: Partial<SessionState> | undefined,
  session: Partial<SessionSummary> | undefined,
): Plan | null {
  if (
    state &&
    (state.loaded || state.streaming || planFromPayload(state as EntityRecord) !== undefined)
  ) {
    return planFromPayloadOr(state as EntityRecord, null)
  }
  const sessionPlan = planFromPayload(session as EntityRecord)
  if (sessionPlan !== undefined) return sessionPlan
  return planFromPayloadOr(state as EntityRecord, null)
}

export function isPlanActive(
  plan: Plan | null | undefined,
  _options: { streaming?: boolean } = {},
) {
  return Boolean(plan?.items?.length)
}
