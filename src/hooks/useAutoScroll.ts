// 自动滚动 hook：内容增长时保持在底部，用户向上翻时暂停并标记未读。
// 程序化滚动用专门的标记窗口（两帧后清除）与用户滚动区分开——否则
// scrollTo 触发的滚动事件会被误判为“用户向上滚动”，导致贴底状态被
// 错误翻转、自动滚动永久失效。滚动跳帧合并在 rAF 内，减少远程桌面闪烁。
import { useCallback, useEffect, useRef, useState, type UIEvent } from 'react'

// 自动滚动：contentVersion 变化且贴底时滚到底部；
// 用户上翻后暂停并置未读标记；程序化滚动用两帧标记窗口区分于用户滚动。
export function useAutoScroll(
  contentVersion: unknown,
  { threshold = 64 }: { threshold?: number } = {},
) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const [scrollElement, setScrollElement] = useState<HTMLDivElement | null>(null)
  const setScrollRef = useCallback((node: HTMLDivElement | null) => {
    scrollRef.current = node
    setScrollElement(node)
  }, [])
  const frameRef = useRef(0)
  // 标记由 scrollToBottom 触发的滚动窗口，而不是用户操作；scrollTo 是异步的，可能连续触发多个事件，
  // 因此要延迟到后续帧再清除标记。否则内容继续增长时，过期的 scrollHeight 会被误判为用户上翻，
  // 进而永久关闭自动贴底。
  const programmaticScrollRef = useRef(false)
  const programmaticFrameRef = useRef(0)
  const pinnedToBottomRef = useRef(true)
  const [pinnedToBottom, setPinnedToBottom] = useState(true)
  const [hasUnread, setHasUnread] = useState(false)

  const scrollToBottom = useCallback((behavior: ScrollBehavior = 'auto') => {
    const node = scrollRef.current
    if (!node) return
    programmaticScrollRef.current = true
    cancelAnimationFrame(programmaticFrameRef.current)
    node.scrollTo({ top: node.scrollHeight, behavior })
    pinnedToBottomRef.current = true
    setPinnedToBottom(true)
    setHasUnread(false)
    // 虚拟化行可能在 ResizeObserver 回调后一帧才稳定，因此让程序化滚动标记覆盖测量帧，
    // 避免这段布局调整被误判为用户向上滚动。
    programmaticFrameRef.current = requestAnimationFrame(() => {
      programmaticFrameRef.current = requestAnimationFrame(() => {
        programmaticScrollRef.current = false
      })
    })
  }, [])

  const cancelProgrammaticScroll = useCallback(() => {
    cancelAnimationFrame(programmaticFrameRef.current)
    programmaticScrollRef.current = false
  }, [])

  const onScroll = useCallback(
    (event: UIEvent<HTMLDivElement>) => {
      const node = event.currentTarget
      if (programmaticScrollRef.current) return
      const pinned = node.scrollHeight - node.scrollTop - node.clientHeight <= threshold
      pinnedToBottomRef.current = pinned
      setPinnedToBottom(pinned)
      if (pinned) setHasUnread(false)
    },
    [threshold],
  )

  // 统一的滚动提交点：contentVersion 变化与 maintainBottom（totalSize 变化）
  // 都经这里合并到每帧一次 scrollTo，避免两条路径在同一帧内重复滚动。
  const scheduleScrollToBottom = useCallback(() => {
    cancelAnimationFrame(frameRef.current)
    frameRef.current = requestAnimationFrame(() => scrollToBottom())
  }, [scrollToBottom])

  useEffect(() => {
    if (!scrollRef.current) return
    if (!pinnedToBottom) {
      setHasUnread(true)
      return undefined
    }
    // 将滚动跳转合并到每帧一次，减少远程桌面等环境中的闪烁。
    scheduleScrollToBottom()
    return () => cancelAnimationFrame(frameRef.current)
  }, [contentVersion, pinnedToBottom, scheduleScrollToBottom])

  const maintainBottom = useCallback(() => {
    if (!pinnedToBottomRef.current) return
    // 内容尺寸变化时先屏蔽同一帧的布局滚动事件，避免流式行变高被误判为用户上翻。
    programmaticScrollRef.current = true
    scheduleScrollToBottom()
  }, [scheduleScrollToBottom])

  // 卸载时清理尚未执行的程序化滚动标记复位。
  useEffect(() => () => cancelAnimationFrame(programmaticFrameRef.current), [])

  return {
    scrollRef,
    scrollElement,
    setScrollRef,
    onScroll,
    hasUnread,
    scrollToBottom,
    maintainBottom,
    cancelProgrammaticScroll,
  }
}
