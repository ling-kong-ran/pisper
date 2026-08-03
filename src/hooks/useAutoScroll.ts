import { useCallback, useEffect, useRef, useState, type UIEvent } from 'react'

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
  // Marks a window during which scroll events come from our own scrollToBottom,
  // not the user. scrollTo fires asynchronously and may emit several scroll
  // events; the flag is cleared on the next frame so all of them are ignored.
  // Without this, a programmatic scroll whose scrollHeight was stale (content
  // kept growing right after) would measure a large gap and wrongly flip
  // pinnedToBottom to false, permanently disabling auto-scroll.
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
    // Virtualized rows can settle one frame after ResizeObserver fires. Keep the
    // programmatic marker through that measurement frame so it cannot look like
    // an upward user scroll.
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

  useEffect(() => {
    if (!scrollRef.current) return
    if (!pinnedToBottom) {
      setHasUnread(true)
      return undefined
    }
    // Coalesce scroll jumps to one per animation frame to reduce remote-desktop flicker.
    cancelAnimationFrame(frameRef.current)
    frameRef.current = requestAnimationFrame(() => scrollToBottom())
    return () => cancelAnimationFrame(frameRef.current)
  }, [contentVersion, pinnedToBottom, scrollToBottom])

  const maintainBottom = useCallback(() => {
    if (pinnedToBottomRef.current) scrollToBottom()
  }, [scrollToBottom])

  // Clean up any pending programmatic-scroll marker reset on unmount.
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
