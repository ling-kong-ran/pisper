// 会话状态（SessionState）的纯函数工具集，供聊天页与流式同步共用：
// - applySessionUpdate 用浅比较去重，未变化的更新返回原引用避免触发重渲染；
// - insertInteractiveUserMessage：运行中追加的消息还没送达时排在正在写的回复后面（参考 Claude），
//   已送达的插到正在写的回复之前；deliverQueuedInputs 在送达那一刻把已写出的回复定格成一段；
// - resolveSessionPlan 优先实时会话状态，显式 null 表示已清空、不得回退到
//   列表快照里的过期 plan。
import { planFromPayload, planFromPayloadOr } from '@/lib/plan-protocol'
import type { ChatMessage, EntityRecord, Plan, SessionState, SessionSummary } from '@/types/chat'

export const DEFAULT_SESSION_STATE: SessionState = Object.freeze({
  messages: [],
  tools: [],
  approvals: [],
  questions: [],
  agents: [],
  team: null,
  currentActivity: null,
  activityFeed: [],
  lifecycle: null,
  sessionTreeRevision: 0,
  thinkingText: '',
  queuedInputs: [],
  hadQueuedInput: false,
  plan: null,
  executionMode: null,
  runMode: null,
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

type QueuedInputSnapshot = {
  queuedInputs?: EntityRecord[] | null
  queueRevision?: number
  removedInputId?: string
}

export function isQueuedInputSnapshotStale(current: SessionState, snapshot: QueuedInputSnapshot) {
  return (
    Number.isSafeInteger(current.queueRevision) &&
    Number.isSafeInteger(snapshot.queueRevision) &&
    snapshot.queueRevision! < current.queueRevision
  )
}

// HTTP 与 SSE 共用修订号；撤回身份不依赖文本，迟到的撤回事件仍能清除对应临时气泡。
export function reconcileQueuedInputSnapshot(
  current: SessionState,
  snapshot: QueuedInputSnapshot,
): SessionState {
  const withdrawnInputIds: string[] = snapshot.removedInputId
    ? [...new Set([...(current.withdrawnInputIds || []), snapshot.removedInputId])]
    : current.withdrawnInputIds || []
  const withdrawn = new Set(withdrawnInputIds)
  const messages = withdrawn.size
    ? current.messages.filter((message) => !withdrawn.has(message.queuedInputId ?? ''))
    : current.messages
  if (isQueuedInputSnapshotStale(current, snapshot))
    return { ...current, messages, withdrawnInputIds }
  const queuedInputs = resolveQueuedInputs(current.queuedInputs, snapshot.queuedInputs)
  return {
    ...current,
    messages,
    withdrawnInputIds,
    queuedInputs: withdrawn.size
      ? queuedInputs.filter((item) => !withdrawn.has(item.id))
      : queuedInputs,
    ...(Number.isSafeInteger(snapshot.queueRevision)
      ? { queueRevision: snapshot.queueRevision }
      : {}),
    hadQueuedInput: Boolean(current.hadQueuedInput || queuedInputs.length),
  }
}

// 还没送达代理的排队消息（手机上是标着“排队中”的气泡，桌面在排队列表里）。
export function isPendingQueuedMessage(message: ChatMessage | undefined) {
  return message?.role === 'user' && message.queuedPending === true
}

// 时间线上最后一条“已发生”的消息：跳过排在末尾、还没送达的排队消息，以及 Bot 派活的事件条目
// （它们按时间插在对话里，但不是回复）。正在写的回复就是它。
export function lastSettledMessageIndex(messages: readonly ChatMessage[] = []) {
  let index = messages.length - 1
  while (
    index >= 0 &&
    (isPendingQueuedMessage(messages[index]) || messages[index]?.role === 'event')
  )
    index -= 1
  return index
}

// 把交互式用户消息插入消息列表（参考 Claude）：
// - 还没送达的排队消息排在末尾（正在写的回复后面），送达时再挪进时间线；
// - 已送达的（或不是排队来的）插到“最近一条 agent 消息”之前，出现在旧回复与后续流式输出之间。
export function insertInteractiveUserMessage(
  messages: ChatMessage[] = [],
  message: ChatMessage,
): ChatMessage[] {
  const current = Array.isArray(messages) ? messages : []
  if (isPendingQueuedMessage(message)) return [...current, message]
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

// 排队消息送达代理（Pi 在下一步开头读到它）：正在写的回复已写出的部分定格成一段留在原位
// （被“立即发送”打断的标 interrupted），送达的消息接在它后面，正在写的回复从空白接着往下写。
// 与 Claude 一样，时间线只往下追加，不会把已经显示的内容换掉。
export function deliverQueuedInputs(
  current: SessionState,
  {
    agentId,
    deliveredIds,
    frozenText,
    interrupted,
  }: { agentId: string; deliveredIds: readonly string[]; frozenText: string; interrupted: boolean },
): SessionState {
  if (!deliveredIds.length) return current
  const ids = new Set(deliveredIds)
  const isDelivered = (message: ChatMessage) =>
    message.role === 'user' &&
    typeof message.queuedInputId === 'string' &&
    ids.has(message.queuedInputId)
  const delivered = current.messages
    .filter(isDelivered)
    .map((message) => ({ ...message, queuedPending: false }))
  const rest = current.messages.filter((message) => !isDelivered(message))
  const deliveredInputIds = [...new Set([...(current.deliveredInputIds || []), ...deliveredIds])]
  const liveIndex = rest.findIndex((message) => message.id === agentId)
  if (liveIndex < 0) return { ...current, deliveredInputIds }
  const live = rest[liveIndex]
  const parts = rest.filter((message) => message.id.startsWith(`${agentId}-part-`)).length
  const frozen: ChatMessage[] = frozenText
    ? [
        {
          id: `${agentId}-part-${parts + 1}`,
          role: 'agent',
          text: frozenText,
          streaming: false,
          ...(interrupted ? { interrupted: true } : {}),
        },
      ]
    : []
  return {
    ...current,
    deliveredInputIds,
    messages: [
      ...rest.slice(0, liveIndex),
      ...frozen,
      ...delivered,
      frozenText ? { ...live, text: '' } : live,
      ...rest.slice(liveIndex + 1),
    ],
  }
}

// 会话仍处“运行中”（流式输出、恢复中或有排队/启动/运行的 agent）时，
// 即使面板被关闭也应保留会话状态，供重新打开时无缝续显。
export function shouldRetainClosedSessionState(state: Partial<SessionState> | undefined) {
  return Boolean(
    state?.streaming ||
    state?.recovering ||
    state?.pendingRuntimeSelection ||
    state?.switchingModel ||
    state?.switchingThinking ||
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
      activity.activityFeed?.some((item: EntityRecord) => item.type === 'agent') ||
      activity.team),
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
