import type { ChatMessage } from '@/types/chat'
import { abortReason, throwIfAborted } from '@/lib/abort-signal'

export type VoiceResponseUpdate = {
  sessionId: string
  messageId: string
  prompt?: string
  text: string
  status: 'started' | 'recovering' | 'streaming' | 'completed' | 'failed'
  source?: 'snapshot'
  runId?: string
  startedAt?: string
  users?: ChatMessage[]
  error?: string
}

type Listener = (update: VoiceResponseUpdate) => void
const listeners = new Map<string, Set<Listener>>()

export function subscribeVoiceResponse(sessionId: string, listener: Listener) {
  const group = listeners.get(sessionId) ?? new Set<Listener>()
  group.add(listener)
  listeners.set(sessionId, group)
  return () => {
    group.delete(listener)
    if (!group.size) listeners.delete(sessionId)
  }
}

// 沿用已鉴权、已检查所有权的 SSE；不另开连接，也不等待打字机动画回灌。
export function publishVoiceResponse(update: VoiceResponseUpdate) {
  for (const listener of listeners.get(update.sessionId) ?? []) listener(update)
}

// 只转发已被会话同步层接受的快照；startedAt 必须由消费者与本轮 SSE meta 严格对照。
export function publishVoiceSnapshot(
  sessionId: string,
  snapshot: {
    startedAt?: string | null
    streaming?: boolean
    error?: string
    messages: ChatMessage[]
  },
) {
  let userIndex = snapshot.messages.length - 1
  while (userIndex >= 0 && snapshot.messages[userIndex].role !== 'user') userIndex -= 1
  const replies = snapshot.messages
    .slice(userIndex + 1)
    .filter((message) => message.role === 'agent' || message.role === 'assistant')
  const reply = replies.find((message) => message.streaming) ?? replies.at(-1)
  publishVoiceResponse({
    sessionId,
    messageId: reply?.id ?? '',
    prompt: snapshot.messages[userIndex]?.text,
    startedAt: snapshot.startedAt ?? undefined,
    source: 'snapshot',
    users: snapshot.messages.filter((message) => message.role === 'user'),
    text: reply?.text ?? '',
    status:
      snapshot.error || reply?.error
        ? 'failed'
        : snapshot.streaming
          ? reply
            ? 'streaming'
            : 'started'
          : 'completed',
    error: snapshot.error || reply?.error,
  })
}

export function createVoiceTextStream(signal: AbortSignal) {
  let snapshot = ''
  let consumed = 0
  let ended = false
  let failure: unknown
  let wake: (() => void) | undefined
  const notify = () => {
    wake?.()
    wake = undefined
  }
  const abort = () => {
    failure = abortReason(signal)
    notify()
  }
  signal.addEventListener('abort', abort, { once: true })
  if (signal.aborted) abort()
  return {
    get text() {
      return snapshot
    },
    update(text: string) {
      if (ended || failure) return
      if (text.length > 32_000) throw new Error('Speech text exceeds the supported limit.')
      // 已交给播放器的内容不能撤回；拒绝重写，避免重复或继续播报陈旧回答。
      if (!text.startsWith(snapshot)) throw new Error('The streamed speech response was rewritten.')
      snapshot = text
      notify()
    },
    finish(error?: unknown) {
      if (ended && error === undefined) return
      ended = true
      if (error !== undefined) failure = error
      notify()
    },
    async *[Symbol.asyncIterator]() {
      try {
        for (;;) {
          throwIfAborted(signal)
          if (failure) throw failure
          if (consumed < snapshot.length) {
            const delta = snapshot.slice(consumed)
            consumed = snapshot.length
            yield delta
          } else if (ended) return
          else
            await new Promise<void>((resolve) => {
              wake = resolve
            })
        }
      } finally {
        signal.removeEventListener('abort', abort)
        ended = true
        notify()
      }
    },
  }
}
