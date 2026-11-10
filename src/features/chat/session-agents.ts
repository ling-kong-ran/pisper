// 会话派出的子 agent：Bot 的对话把它们当作时间线里的事件（“派了：补退款测试 · 在跑 2:10”），
// 在跑的时候按状态刷新，可以展开看最近的输出，也可以停掉。数据来自 /api/sessions/:id/agents。
import { createContext, useCallback, useEffect, useRef, useState } from 'react'
import { consumeEventStream } from '@/lib/event-stream'
import { requestJson } from '@/lib/http'
import type { ChatMessage, EntityRecord } from '@/types/chat'

// 运行时派的只读巡看（runtime/services/bots/bot-proactive.mjs 的 BOT_PROACTIVE_TASK_NAME）。
export const PROACTIVE_TASK_NAME = 'proactive_check'

export type SessionAgentStatus =
  'queued' | 'starting' | 'running' | 'completed' | 'failed' | 'interrupted'

export type SessionAgent = {
  id: string
  // agent = 派出去的子 agent；command = Bot 对话转到后台接着跑的命令。
  kind: 'agent' | 'command' | 'proactive'
  taskName: string
  status: SessionAgentStatus
  message: string
  model: string
  output: string
  error: string
  startedAt: string
  completedAt: string | null
  toolCallCount: number
  activity: string
  // 派出它的那次 spawn_agent 调用（旧 Runtime 不带）：父会话的工具行据此找到对应的子代理。
  toolCallId: string
}

const STATUSES: readonly SessionAgentStatus[] = [
  'queued',
  'starting',
  'running',
  'completed',
  'failed',
  'interrupted',
]

export function isActiveAgent(agent: Pick<SessionAgent, 'status'>) {
  return agent.status === 'queued' || agent.status === 'starting' || agent.status === 'running'
}

function text(value: unknown, max = 500) {
  return typeof value === 'string' ? value.slice(0, max) : ''
}

// 按不可信数据解析：缺 id 或状态不认识的条目丢掉，其余字段给安全默认值。
export function parseSessionAgents(value: unknown): SessionAgent[] {
  const list =
    value && typeof value === 'object' ? (value as Record<string, unknown>).agents : undefined
  if (!Array.isArray(list)) return []
  return list.flatMap((item) => {
    if (!item || typeof item !== 'object') return []
    const input = item as Record<string, unknown>
    const status = STATUSES.find((entry) => entry === input.status)
    if (typeof input.id !== 'string' || !input.id || !status) return []
    const activity =
      input.currentActivity && typeof input.currentActivity === 'object'
        ? (input.currentActivity as Record<string, unknown>)
        : {}
    const toolCalls = Number(input.toolCallCount)
    return [
      {
        id: input.id,
        // 运行时派的只读巡看不带标题，按任务 id 认出来，界面显示本地化的名字。
        kind:
          input.kind === 'command'
            ? 'command'
            : !text(input.title, 120) && input.taskName === PROACTIVE_TASK_NAME
              ? 'proactive'
              : 'agent',
        // 人看的标题优先（Bot 派活时写的），没有时用规整过的 taskName。
        taskName: text(input.title, 120) || text(input.taskName, 120) || 'agent',
        status,
        message: text(input.message),
        model: text(input.model, 120),
        output: text(input.output),
        error: text(input.error),
        startedAt: text(input.startedAt, 40),
        completedAt: text(input.completedAt, 40) || null,
        toolCallCount: Number.isInteger(toolCalls) && toolCalls > 0 ? toolCalls : 0,
        activity: text(activity.name, 80) || text(activity.stage, 40),
        toolCallId: text(input.toolCallId, 128),
      },
    ]
  })
}

// Bot 之间的来往：它找别的 Bot（“在等 测试 回话”），或者来往太多停下来了。
export type SessionBotLinkStatus = 'waiting' | 'answered' | 'stopped'

export type SessionBotLink = {
  id: string
  botId: string
  direction: 'out' | 'in'
  text: string
  status: SessionBotLinkStatus
  reason: string
  openedAt: string
}

const LINK_STATUSES: readonly SessionBotLinkStatus[] = ['waiting', 'answered', 'stopped']

export function parseSessionLinks(value: unknown): SessionBotLink[] {
  const list =
    value && typeof value === 'object' ? (value as Record<string, unknown>).links : undefined
  if (!Array.isArray(list)) return []
  return list.flatMap((item) => {
    if (!item || typeof item !== 'object') return []
    const input = item as Record<string, unknown>
    const status = LINK_STATUSES.find((entry) => entry === input.status)
    if (typeof input.id !== 'string' || !input.id || typeof input.botId !== 'string' || !status)
      return []
    return [
      {
        id: input.id,
        botId: input.botId,
        direction: input.direction === 'in' ? 'in' : 'out',
        text: text(input.text, 300),
        status,
        reason: text(input.reason, 20),
        openedAt: text(input.openedAt, 40),
      },
    ]
  })
}

export function fetchSessionAgents(sessionId: string, signal?: AbortSignal) {
  return requestJson(`/api/sessions/${encodeURIComponent(sessionId)}/agents`, {
    signal,
    parse: (value) => ({ agents: parseSessionAgents(value), links: parseSessionLinks(value) }),
  })
}

// 一个子代理的过程（只读）：主代理交给它的任务、和普通会话同样形状的消息。
// available 为假：这个子代理是在保存过程之前派出的，没有留下记录。
export type AgentTranscript = {
  available: boolean
  active: boolean
  task: string
  // 第一条消息就是任务本身（界面已经放在“任务”块里，不再当成一条发言显示）。
  taskShown: boolean
  omitted: number
  messages: ChatMessage[]
}

export function parseAgentTranscript(value: unknown): AgentTranscript {
  const input = value && typeof value === 'object' ? (value as Record<string, unknown>) : {}
  const messages = (Array.isArray(input.messages) ? input.messages : []).filter(
    (item): item is ChatMessage =>
      Boolean(item) &&
      typeof item === 'object' &&
      typeof (item as Record<string, unknown>).id === 'string' &&
      typeof (item as Record<string, unknown>).role === 'string',
  )
  const omitted = Number(input.omitted)
  return {
    available: input.available === true,
    active: input.active === true,
    task: text(input.task, 12_000),
    taskShown: input.taskShown === true,
    omitted: Number.isInteger(omitted) && omitted > 0 ? omitted : 0,
    messages,
  }
}

export function fetchAgentTranscript(sessionId: string, agentId: string, signal?: AbortSignal) {
  return requestJson(
    `/api/sessions/${encodeURIComponent(sessionId)}/agents/${encodeURIComponent(agentId)}/transcript`,
    { signal, parse: parseAgentTranscript },
  )
}

// 推送的一帧：完整的（带 messages），或只带 from 之后几条的（tail），用上一份的前面接上。
// 接不上（还没有上一份、from 越界）时返回 null，调用方重连拿一份完整的。
export function applyAgentTranscriptFrame(
  previous: AgentTranscript | null,
  value: unknown,
): AgentTranscript | null {
  const input = value && typeof value === 'object' ? (value as Record<string, unknown>) : {}
  if (!Array.isArray(input.tail)) return parseAgentTranscript(value)
  const from = Number(input.from)
  if (!previous || !Number.isInteger(from) || from < 0 || from > previous.messages.length)
    return null
  const next = parseAgentTranscript({ ...input, messages: input.tail })
  return { ...next, messages: [...previous.messages.slice(0, from), ...next.messages] }
}

// 子代理过程的推送：每收到一帧交给 onTranscript；子代理结束（done）或连接断开时返回。
// 返回最后一份，调用方据此决定要不要重连（还在跑就重连）。
export async function streamAgentTranscript(
  sessionId: string,
  agentId: string,
  onTranscript: (transcript: AgentTranscript) => void,
  signal: AbortSignal,
): Promise<AgentTranscript | null> {
  const response = await fetch(
    `/api/sessions/${encodeURIComponent(sessionId)}/agents/${encodeURIComponent(agentId)}/transcript/events`,
    { signal, headers: { Accept: 'text/event-stream' } },
  )
  let current = null as AgentTranscript | null
  await consumeEventStream<unknown>(response, (event, data) => {
    if (event === 'done') return false
    if (event !== 'transcript') return true
    const next = applyAgentTranscriptFrame(current, data)
    if (!next) return false
    current = next
    onTranscript(next)
    return true
  })
  return current
}

export function stopSessionAgent(sessionId: string, agentId: string) {
  return requestJson(
    `/api/sessions/${encodeURIComponent(sessionId)}/agents/${encodeURIComponent(agentId)}/stop`,
    { method: 'POST', data: {} },
  )
}

// 时间线渲染事件条目时用：这个会话的子 agent（按 id）与刷新。Bot 的会话页提供，其他页面没有。
export type AgentJobsValue = {
  sessionId: string
  agents: ReadonlyMap<string, SessionAgent>
  links: ReadonlyMap<string, SessionBotLink>
  reload: () => void
}

export const AgentJobsContext = createContext<AgentJobsValue | null>(null)

// 会话页的子代理（只在 Runtime 能给出子代理过程时提供）：工具组里的子代理行、运行中弹层与 Bot 工作卡
// 据此显示子代理自己的状态，并在检查器的“子代理”标签里打开它的过程。
export type SessionSubagentsValue = {
  sessionId: string
  agents: readonly SessionAgent[]
  open: (agentId: string) => void
}

export const SessionSubagentsContext = createContext<SessionSubagentsValue | null>(null)

// 这个会话派过子代理吗（历史回复的工具活动或当前运行里出现过 spawn_agent / 子代理活动）：
// 派过才去拉子代理列表，普通会话不多发请求。
export function sessionHasSubagents(
  messages: readonly ChatMessage[],
  live: readonly EntityRecord[],
) {
  const spawned = (activity: unknown) => {
    const entry = activity && typeof activity === 'object' ? (activity as EntityRecord) : null
    return Boolean(entry && (entry.type === 'agent' || entry.name === 'spawn_agent'))
  }
  if (live.some(spawned)) return true
  return messages.some((message) => {
    const run = message.runActivity as EntityRecord | undefined
    return Array.isArray(run?.tools) && run.tools.some(spawned)
  })
}

// 工具组里一次 spawn_agent 调用对应的子代理：新 Runtime 按调用 id 对上；旧记录没有调用 id 时按任务名找最近一个。
export function agentForSpawnCall(
  agents: readonly SessionAgent[],
  toolCallId: string,
  taskName: string,
) {
  return (
    (toolCallId ? agents.find((agent) => agent.toolCallId === toolCallId) : undefined) ??
    (taskName
      ? [...agents].reverse().find((agent) => !agent.toolCallId && agent.taskName === taskName)
      : undefined)
  )
}

const ACTIVE_POLL_MS = 2_500

function sameJson(left: unknown, right: unknown) {
  return JSON.stringify(left) === JSON.stringify(right)
}
const IDLE_POLL_MS = 15_000

// 页面开着时轮询：有在跑的活或在等别的 Bot 回话时 2.5 秒一次，没有时 15 秒一次；换会话或关掉时取消。
// onFinished：有一件活刚结束、或别的 Bot 刚回了话时调用（Bot 随后在后台说一句，页面借此重拉对话）。
export function useSessionAgents(sessionId: string, enabled: boolean, onFinished?: () => void) {
  const [agents, setAgents] = useState<SessionAgent[]>([])
  const [links, setLinks] = useState<SessionBotLink[]>([])
  const [refresh, setRefresh] = useState(0)
  const activeRef = useRef(false)
  const activeIdsRef = useRef<Set<string> | null>(null)
  const knownIdsRef = useRef<Set<string> | null>(null)
  const waitingLinksRef = useRef<Set<string>>(new Set())
  const onFinishedRef = useRef(onFinished)
  onFinishedRef.current = onFinished
  useEffect(() => {
    setAgents([])
    setLinks([])
    activeIdsRef.current = null
    knownIdsRef.current = null
    waitingLinksRef.current = new Set()
  }, [sessionId])
  useEffect(() => {
    if (!enabled || !sessionId) return undefined
    const controller = new AbortController()
    let timer = 0
    let skipped = false
    const load = async () => {
      // 页面看不见时不读；回到前台时补一次（见下面的 visibilitychange）。
      if (document.visibilityState !== 'visible') {
        skipped = true
        timer = window.setTimeout(load, activeRef.current ? ACTIVE_POLL_MS : IDLE_POLL_MS)
        return
      }
      skipped = false
      try {
        const loaded = await fetchSessionAgents(sessionId, controller.signal)
        if (controller.signal.aborted) return
        const next = loaded.agents
        const waiting = new Set(
          loaded.links.filter((link) => link.status === 'waiting').map((link) => link.id),
        )
        activeRef.current = next.some(isActiveAgent) || waiting.size > 0
        const activeIds = new Set(next.filter(isActiveAgent).map((agent) => agent.id))
        const previous = activeIdsRef.current
        const known = knownIdsRef.current
        const answered = [...waitingLinksRef.current].some((id) => !waiting.has(id))
        activeIdsRef.current = activeIds
        knownIdsRef.current = new Set(next.map((agent) => agent.id))
        waitingLinksRef.current = waiting
        // 刚结束的：上次还在跑的不在跑了，或者两次轮询之间就开始又做完了的（快活），或者别的 Bot 回话了。
        if (
          answered ||
          (previous &&
            known &&
            ([...previous].some((id) => !activeIds.has(id)) ||
              next.some((agent) => !known.has(agent.id) && !isActiveAgent(agent))))
        )
          onFinishedRef.current?.()
        // 内容没变就沿用原来的数组：时间线按它重算，不能每 2.5 秒白算一遍。
        setAgents((current) => (sameJson(current, next) ? current : next))
        setLinks((current) => (sameJson(current, loaded.links) ? current : loaded.links))
      } catch {
        // 读不到时保留上一次的结果，下一轮再试。
      }
      if (!controller.signal.aborted)
        timer = window.setTimeout(load, activeRef.current ? ACTIVE_POLL_MS : IDLE_POLL_MS)
    }
    const resume = () => {
      if (!skipped || document.visibilityState !== 'visible') return
      window.clearTimeout(timer)
      void load()
    }
    document.addEventListener('visibilitychange', resume)
    void load()
    return () => {
      controller.abort()
      window.clearTimeout(timer)
      document.removeEventListener('visibilitychange', resume)
    }
  }, [enabled, refresh, sessionId])
  const reload = useCallback(() => setRefresh((value) => value + 1), [])
  return { agents, links, reload }
}

// 时间线里的事件条目：按开始时间插进对话，不算“正在写的回复”（见 lastSettledMessageIndex）。
export function agentEventMessage(agent: SessionAgent): ChatMessage {
  return {
    id: `agent-event-${agent.id}`,
    role: 'event',
    timestamp: agent.startedAt,
    agentId: agent.id,
  }
}

function timeOf(message: ChatMessage) {
  const value = message.timestamp
  if (typeof value === 'number') return value
  const parsed = typeof value === 'string' ? Date.parse(value) : Number.NaN
  return Number.isFinite(parsed) ? parsed : null
}

export function linkEventMessage(link: SessionBotLink): ChatMessage {
  return {
    id: `link-event-${link.id}`,
    role: 'event',
    timestamp: link.openedAt,
    linkId: link.id,
  }
}

// 把事件按时间插进消息列表，原有消息的相对顺序不变。没有时间的消息（刚发出、还没落盘的）沿用前一条的时间，
// 所以比它们晚开始的事件排在它们后面；时间相同时事件排在后面。
export function withAgentEvents(
  messages: ChatMessage[],
  agents: SessionAgent[],
  links: SessionBotLink[] = [],
) {
  if (!agents.length && !links.length) return messages
  const events = [...agents.map(agentEventMessage), ...links.map(linkEventMessage)].sort(
    (left, right) => (timeOf(left) ?? 0) - (timeOf(right) ?? 0),
  )
  const result: ChatMessage[] = []
  let next = 0
  let last = Number.NEGATIVE_INFINITY
  for (const message of messages) {
    last = timeOf(message) ?? last
    while (next < events.length && (timeOf(events[next]) ?? 0) < last) result.push(events[next++])
    result.push(message)
  }
  while (next < events.length) result.push(events[next++])
  return result
}
