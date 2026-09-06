import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createScrollFollowController } from '@/lib/scroll-follow'

export function useAutoScroll(
  contentVersion: unknown,
  { threshold = 64, resetKey }: { threshold?: number; resetKey?: unknown } = {},
) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const [scrollElement, setScrollElement] = useState<HTMLDivElement | null>(null)
  const [hasUnread, setHasUnread] = useState(false)
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
    })
    controllerRef.current = controller
    setHasUnread(false)
    // 首次打开或切换会话直接定位最新消息，只有后续内容增长才做平滑跟随。
    controller.scrollToBottom()
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

  return {
    scrollRef,
    scrollElement,
    setScrollRef,
    hasUnread,
    scrollToBottom,
    maintainBottom,
    pauseFollowing,
  }
}
