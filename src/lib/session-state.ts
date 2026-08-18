// 会话状态（SessionState）的纯函数工具集，供聊天页与流式同步共用：
// - applySessionUpdate 用浅比较去重，未变化的更新返回原引用避免触发重渲染；
// - insertInteractiveUserMessage 把交互消息插到“正在运行的 agent 消息”之前，
//   保持时间线顺序（用户追问应出现在旧回复与后续流式输出之间）；
// - resolveSessionPlan 优先实时会话状态，显式 null 表示已清空、不得回退到
//   列表快照里的过期 plan。
import { planFromPayload, planFromPayloadOr } from '@/lib/plan-protocol'
import type { ChatMessage, EntityRecord, Plan, SessionState, SessionSummary } from '@/types/chat'

export const DEFAULT_SESSION_STATE: SessionState = Object.freeze({
  messages: [],
  tools: [],
  approvals: [],
  agents: [],
  currentActivity: null,
  activityFeed: [],
  lifecycle: null,
  sessionTreeRevision: 0,
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

// 浅比较两个会话状态：任一字段引用不同即视为变化。
// 用于应用更新前的去重——未变化的更新返回原引用，避免无谓重渲染。
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

// 应用一次会话更新：支持部分对象或函数式更新，经过去重后返回新状态；
// 无变化时返回原引用（供 React 状态比较短路）。
export function applySessionUpdate(
  previous: SessionState | undefined,
  update: SessionStateUpdate,
): SessionState {
  const base = previous || DEFAULT_SESSION_STATE
  const next = typeof update === 'function' ? update(base) : { ...base, ...update }
  return sessionStateChanged(base, next) ? next : base
}

// 解析排队输入：incoming 为 undefined 表示“未提及”，保留现有值；
// 为 null/数组时直接替换，保证 SSE 流里显式清空（[]）能生效。
export function resolveQueuedInputs(
  current: EntityRecord[] | undefined,
  incoming: EntityRecord[] | null | undefined,
) {
  if (incoming === undefined) return current || []
  return Array.isArray(incoming) ? incoming : []
}

// 把交互式用户消息插入消息列表：插到“最近一条 agent 消息”之前。
// 这样用户追问出现在旧回复与后续流式输出之间，时间线顺序正确；
// 没有活跃 agent 消息时直接追加到末尾。
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

// 会话仍处“运行中”（流式输出、恢复中或有排队/启动/运行的 agent）时，
// 即使面板被关闭也应保留会话状态，供重新打开时无缝续显。
export function shouldRetainClosedSessionState(state: Partial<SessionState> | undefined) {
  return Boolean(
    state?.streaming ||
    state?.recovering ||
    state?.agents?.some((agent) => ['queued', 'starting', 'running'].includes(agent.status)),
  )
}

// 判断活动记录是否包含可展示的运行内容（有流式文本/思考/工具/agent 活动），
// 避免渲染只有空壳的活动卡片。
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

// 消息的运行活动解析：最新一条 agent 消息优先展示实时活动（流式中的
// 工具/思考），否则回退到消息自带的 runActivity 快照。
export function resolveMessageRunActivity(
  message: ChatMessage,
  isLatestAgent: boolean,
  liveActivity: EntityRecord | null | undefined,
) {
  if (isLatestAgent && hasRunActivity(liveActivity)) return liveActivity
  return hasRunActivity(message.runActivity) ? message.runActivity : null
}

/**
 * 计划来源解析：会话打开后优先采用实时会话状态；
 * 显式 null 表示“已清空”，不得回退到过期的 listSessions 快照。
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

// 判断计划是否“活跃”（有待办项）：仅用于 UI 是否显示计划卡片。
export function isPlanActive(
  plan: Plan | null | undefined,
  _options: { streaming?: boolean } = {},
) {
  return Boolean(plan?.items?.length)
}
