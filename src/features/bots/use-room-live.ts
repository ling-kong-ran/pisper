// 群聊页的数据：群详情（成员状态、简报）与消息。先读最后一页，之后轮询“某条之后的”增量
// （同时带回还没做完的工作卡，状态会变）；有 Bot 在干活时 1.5 秒一次，否则 5 秒；页面隐藏时不刷。
// 请求按群切换或卸载时取消；乱序到达的旧群响应丢弃。
import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchRoom, fetchRoomMessages, type RoomDetail, type RoomMessage } from './rooms-api'

function merge(current: RoomMessage[], incoming: readonly RoomMessage[]): RoomMessage[] {
  if (!incoming.length) return current
  const byId = new Map(current.map((message) => [message.id, message]))
  for (const message of incoming) byId.set(message.id, message)
  return [...byId.values()].sort((left, right) => left.seq - right.seq)
}

export function useRoomLive(roomId: string) {
  const [detail, setDetail] = useState<RoomDetail | null>(null)
  const [messages, setMessages] = useState<RoomMessage[]>([])
  const [hasMore, setHasMore] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const lastSeq = useRef(0)
  const generation = useRef(0)
  const polling = useRef<Promise<void> | null>(null)

  const busy =
    (detail?.activity.working ?? 0) > 0 ||
    messages.some(
      (message) => message.turn?.state === 'running' || message.turn?.state === 'waiting',
    )

  const poll = useCallback(async () => {
    const current = generation.current
    polling.current ??= Promise.all([
      fetchRoom(roomId),
      fetchRoomMessages(roomId, { after: lastSeq.current }),
    ])
      .then(([room, page]) => {
        if (current !== generation.current) return
        setDetail(room)
        setMessages((previous) => merge(previous, page.messages))
        lastSeq.current = Math.max(lastSeq.current, page.lastSeq)
        setError('')
      })
      .catch((caught: unknown) => {
        if (current === generation.current)
          setError(caught instanceof Error ? caught.message : String(caught))
      })
      .finally(() => {
        polling.current = null
      })
    return polling.current
  }, [roomId])

  // 换群：清空、读详情与最后一页。
  useEffect(() => {
    generation.current += 1
    const current = generation.current
    const controller = new AbortController()
    setDetail(null)
    setMessages([])
    setHasMore(false)
    setLoading(true)
    setError('')
    lastSeq.current = 0
    Promise.all([
      fetchRoom(roomId, controller.signal),
      fetchRoomMessages(roomId, {}, controller.signal),
    ])
      .then(([room, page]) => {
        if (current !== generation.current) return
        setDetail(room)
        setMessages(page.messages)
        setHasMore(page.hasMore)
        lastSeq.current = page.lastSeq
      })
      .catch((caught: unknown) => {
        if (current === generation.current && !controller.signal.aborted)
          setError(caught instanceof Error ? caught.message : String(caught))
      })
      .finally(() => {
        if (current === generation.current) setLoading(false)
      })
    return () => controller.abort()
  }, [roomId])

  useEffect(() => {
    if (loading) return
    const timer = window.setInterval(
      () => {
        if (document.visibilityState === 'visible') void poll()
      },
      busy ? 1_500 : 5_000,
    )
    return () => window.clearInterval(timer)
  }, [busy, loading, poll])

  const loadOlder = useCallback(async () => {
    const first = messages[0]
    if (!first) return
    const current = generation.current
    const page = await fetchRoomMessages(roomId, { before: first.seq })
    if (current !== generation.current) return
    setMessages((previous) => merge(previous, page.messages))
    setHasMore(page.hasMore)
  }, [messages, roomId])

  // 本地先换掉一条（撤回后的占位、刚发出的消息），不等下一次轮询。不动 lastSeq：
  // 它前面可能还有没取到的消息（比如同时贴出的工作卡），下一次轮询照常从旧位置取。
  const replace = useCallback((message: RoomMessage) => {
    setMessages((previous) => merge(previous, [message]))
  }, [])

  return { detail, messages, hasMore, loading, error, busy, poll, loadOlder, replace, setDetail }
}
