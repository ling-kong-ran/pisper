// 群聊页的数据：群详情（成员状态、简报）与消息。先读最后一页，之后取“某条之后的”增量
// （同时带回还没做完的工作卡，状态会变）。Runtime 推 room 帧（rooms.events）时收到这个群的帧就取，
// 只留 25 秒一次的兜底轮询；旧 Runtime 或事件流断了时照旧轮询：有 Bot 在干活时 1.5 秒一次，否则 5 秒。
// 页面隐藏时不刷，回到前台、事件流重连时立刻补一次。请求按群切换或卸载时取消；乱序到达的旧群响应丢弃。
import { useCallback, useEffect, useRef, useState } from 'react'
import { retainBoardSync } from '@/features/board/public'
import { roomFeedAlive, subscribeRoomChanges } from '@/features/board/room-changes'
import { ApiError } from '@/lib/api-error'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { ROOM_BUSY_POLL_MS, roomChangeAffectsPage, roomPollDelay } from './room-live-policy'
import { fetchRoom, fetchRoomMessages, type RoomDetail, type RoomMessage } from './rooms-api'

// 合并一批消息。内容没变的那条沿用原来的对象，一条都没变时沿用原来的数组：
// 轮询每一两秒来一次，不能每次都让整条时间线（连同 Markdown）重算。
function merge(
  current: RoomMessage[],
  incoming: readonly RoomMessage[],
  recalled: ReadonlySet<string>,
): RoomMessage[] {
  if (!incoming.length) return current
  const byId = new Map(current.map((message) => [message.id, message]))
  let changed = false
  for (const message of incoming) {
    // 刚撤回的那条：撤回之前发出的请求晚到时还带着原文，不让它盖回来。
    if (recalled.has(message.id) && !message.recalled) continue
    const previous = byId.get(message.id)
    if (previous && JSON.stringify(previous) === JSON.stringify(message)) continue
    byId.set(message.id, message)
    changed = true
  }
  return changed ? [...byId.values()].sort((left, right) => left.seq - right.seq) : current
}

function sameDetail(left: RoomDetail | null, right: RoomDetail) {
  return Boolean(left) && JSON.stringify(left) === JSON.stringify(right)
}

function isLive(message: RoomMessage) {
  return message.turn?.state === 'running' || message.turn?.state === 'waiting'
}

export function useRoomLive(roomId: string) {
  const [detail, setDetail] = useState<RoomDetail | null>(null)
  const [messages, setMessages] = useState<RoomMessage[]>([])
  const [hasMore, setHasMore] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  // 群不在了（在别处被删、Bots 实验功能关了）：页面据此退回 Bots 页，不再“正在重连”。
  const [gone, setGone] = useState(false)
  const lastSeq = useRef(0)
  const generation = useRef(0)
  const polling = useRef<Promise<void> | null>(null)
  // 正在取的时候又来了一帧：这次取完再取一次，不漏掉取的过程中发生的变化。
  const again = useRef(false)
  const lastPollAt = useRef(0)
  const recalled = useRef(new Set<string>())
  const shown = useRef<RoomMessage[]>([])
  const soon = useRef(0)
  shown.current = messages

  const busy = (detail?.activity.working ?? 0) > 0 || messages.some(isLive)
  const push = useRuntimeCapabilitiesStore(
    (state) => state.capabilities.features['rooms.events'] === true,
  )
  // room 帧走看板事件流：页面开着时持有看板同步（手机上侧栏不在时也连着）；没有推送时不必持有。
  useEffect(() => (push ? retainBoardSync() : undefined), [push])

  const poll = useCallback(async (): Promise<void> => {
    const current = generation.current
    let more = false
    if (polling.current) {
      again.current = true
      return polling.current
    }
    lastPollAt.current = Date.now()
    polling.current = Promise.all([
      fetchRoom(roomId),
      fetchRoomMessages(roomId, {
        after: lastSeq.current,
        // 页面上还显示“在做”的卡：做完后它可能既不在运行、也不是这个 Bot 最近的一张，点名要回来。
        watch: shown.current.filter(isLive).map((message) => message.id),
      }),
    ])
      .then(([room, page]) => {
        if (current !== generation.current) return
        setDetail((previous) => (sameDetail(previous, room) ? previous : room))
        setMessages((previous) => merge(previous, page.messages, recalled.current))
        // 一次最多带回一页：还有更多时只前进到这一页的最后一条，马上接着取，中间不跳过。
        more = page.hasMore && page.messages.length > 0
        const reached = more
          ? Math.max(...page.messages.map((message) => message.seq))
          : page.lastSeq
        lastSeq.current = Math.max(lastSeq.current, reached)
        setError('')
      })
      .catch((caught: unknown) => {
        if (current !== generation.current) return
        if (caught instanceof ApiError && caught.status === 404) setGone(true)
        setError(caught instanceof Error ? caught.message : String(caught))
      })
      .finally(() => {
        polling.current = null
      })
    await polling.current
    const repeat = more || again.current
    again.current = false
    if (repeat && current === generation.current) await poll()
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
    setGone(false)
    lastSeq.current = 0
    recalled.current = new Set()
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
    return () => {
      controller.abort()
      window.clearTimeout(soon.current)
    }
  }, [roomId])

  useEffect(() => {
    if (loading || gone) return
    const visible = () => document.visibilityState === 'visible'
    const now = () => {
      if (visible()) void poll()
    }
    // 定时器按最勤的节奏走，每次看离上次取过了多久：推送在线时只当兜底，断了马上退回勤轮询。
    const tick = () => {
      if (!visible()) return
      const delay = roomPollDelay({ push, alive: roomFeedAlive(), busy })
      if (Date.now() - lastPollAt.current >= delay) void poll()
    }
    const timer = window.setInterval(tick, ROOM_BUSY_POLL_MS)
    document.addEventListener('visibilitychange', now)
    const unsubscribe = push
      ? subscribeRoomChanges((change) => {
          if (roomChangeAffectsPage(change, roomId)) now()
        })
      : () => {}
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', now)
      unsubscribe()
    }
  }, [busy, gone, loading, poll, push, roomId])

  // 现在刷一次，稍后再刷一次：刚发出的话要过一小会儿才贴出 Bot 的工作卡，不等到 5 秒后的下一次。
  const pollSoon = useCallback(() => {
    void poll()
    window.clearTimeout(soon.current)
    soon.current = window.setTimeout(() => void poll(), 700)
  }, [poll])

  const loadOlder = useCallback(async () => {
    const first = shown.current[0]
    if (!first) return
    const current = generation.current
    const page = await fetchRoomMessages(roomId, { before: first.seq })
    if (current !== generation.current) return
    setMessages((previous) => merge(previous, page.messages, recalled.current))
    setHasMore(page.hasMore)
  }, [roomId])

  // 本地先换掉一条（撤回后的占位、刚发出的消息），不等下一次轮询。不动 lastSeq：
  // 它前面可能还有没取到的消息（比如同时贴出的工作卡），下一次轮询照常从旧位置取。
  const replace = useCallback((message: RoomMessage) => {
    if (message.recalled) recalled.current.add(message.id)
    setMessages((previous) => merge(previous, [message], recalled.current))
  }, [])

  return {
    detail,
    messages,
    hasMore,
    loading,
    error,
    gone,
    busy,
    poll,
    pollSoon,
    loadOlder,
    replace,
  }
}
