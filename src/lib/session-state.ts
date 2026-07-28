import type {
  ChatMessage,
  EntityRecord,
  SessionState,
  SessionSummary,
  TaskList,
} from '@/types/chat'

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
  taskList: null,
  executionMode: null,
  contextUsage: null,
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
    if (item?.role === 'agent' && item.streaming) {
      activeAgentIndex = index
      break
    }
  }
  if (activeAgentIndex < 0) return [...current, message]
  return [...current.slice(0, activeAgentIndex), message, ...current.slice(activeAgentIndex)]
}

/**
 * Prefer the live session-state task list once a session has been opened.
 * Important: `null` means “cleared”, and must NOT fall back to stale listSessions data.
 */
export function resolveSessionTaskList(
  state: Partial<SessionState> | undefined,
  session: Partial<SessionSummary> | undefined,
): TaskList | null {
  if (state && (state.loaded || state.streaming || Object.hasOwn(state, 'taskList'))) {
    return state.taskList ?? null
  }
  return session?.taskList ?? state?.taskList ?? null
}

export function isTaskListActive(
  taskList: TaskList | null | undefined,
  { streaming = false }: { streaming?: boolean } = {},
) {
  const items = taskList?.items || []
  if (!items.length) return false
  if (streaming) return true
  return items.some((item) => item?.status !== 'completed')
}
