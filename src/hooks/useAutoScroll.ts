import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createScrollFollowController } from '@/lib/scroll-follow'

export function useAutoScroll(
  contentVersion: unknown,
  {
    threshold = 64,
    resetKey,
    readInitialPosition,
  }: { threshold?: number; resetKey?: unknown; readInitialPosition?: () => number | null } = {},
) {
  const readInitialPositionRef = useRef(readInitialPosition)
  readInitialPositionRef.current = readInitialPosition
  const scrollRef = useRef<HTMLDivElement>(null)
  const [scrollElement, setScrollElement] = useState<HTMLDivElement | null>(null)
  const [hasUnread, setHasUnread] = useState(false)
  // 不在最新消息（读者往上翻了、或回来时停在上次读到的地方）：显示“回到最新”。
  const [awayFromLatest, setAwayFromLatest] = useState(false)
  const controllerRef = useRef<ReturnType<typeof createScrollFollowController> | null>(null)
  const setScrollRef = useCallback((node: HTMLDivElement | null) => {
    scrollRef.current = node
    setScrollElement(node)
  }, [])

  useLayoutEffect(() => {
    if (!scrollElement) return
    const controller = createScrollFollowController(scrollElement, {
      threshold,
      onUnreadChange: setHasUnread,
      onAwayChange: setAwayFromLatest,
    })
    controllerRef.current = controller
    setHasUnread(false)
    setAwayFromLatest(false)
    // 首次打开或切换会话直接定位最新消息，只有后续内容增长才做平滑跟随。
    const position = readInitialPositionRef.current?.()
    if (typeof position === 'number') controller.restorePosition(position)
    else controller.settleAtBottom()
    return () => {
      controller.dispose()
      controllerRef.current = null
    }
  }, [resetKey, scrollElement, threshold])

  useEffect(() => {
    controllerRef.current?.contentChanged()
  }, [contentVersion])

  const scrollToBottom = useCallback((behavior: ScrollBehavior = 'auto') => {
    controllerRef.current?.scrollToBottom(behavior)
  }, [])
  const maintainBottom = useCallback(() => {
    controllerRef.current?.maintainBottom()
  }, [])
  const pauseFollowing = useCallback(() => {
    controllerRef.current?.pauseFollowing()
  }, [])
  const isFollowing = useCallback(() => controllerRef.current?.isFollowing() ?? false, [])

  return {
    scrollRef,
    scrollElement,
    setScrollRef,
    hasUnread,
    awayFromLatest,
    scrollToBottom,
    maintainBottom,
    pauseFollowing,
    isFollowing,
  }
}
