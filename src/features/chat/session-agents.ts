// 会话派出的子 agent：Bot 的对话把它们当作时间线里的事件（“派了：补退款测试 · 在跑 2:10”），
// 在跑的时候按状态刷新，可以展开看最近的输出，也可以停掉。数据来自 /api/sessions/:id/agents。
import { createContext, useCallback, useEffect, useRef, useState } from 'react'
import { requestJson } from '@/lib/http'
import type { ChatMessage } from '@/types/chat'

export type SessionAgentStatus =
  'queued' | 'starting' | 'running' | 'completed' | 'failed' | 'interrupted'

export type SessionAgent = {
  id: string
  // agent = 派出去的子 agent；command = Bot 对话转到后台接着跑的命令。
  kind: 'agent' | 'command'
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
        kind: input.kind === 'command' ? 'command' : 'agent',
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

const ACTIVE_POLL_MS = 2_500
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
    const load = async () => {
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
        setAgents(next)
        setLinks(loaded.links)
      } catch {
        // 读不到时保留上一次的结果，下一轮再试。
      }
      if (!controller.signal.aborted)
        timer = window.setTimeout(load, activeRef.current ? ACTIVE_POLL_MS : IDLE_POLL_MS)
    }
    void load()
    return () => {
      controller.abort()
      window.clearTimeout(timer)
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
