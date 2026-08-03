import { memo, useCallback, useLayoutEffect, useRef, useState, type RefObject } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { resolveMessageRunActivity } from '@/lib/session-state'
import type { ChatMessage } from '@/types/chat'
import type { AgentRunActivityProps } from './AgentRunActivity'
import { FocusChatMessage } from './ChatMessage'
import { TRANSCRIPT_ESTIMATED_ROW_HEIGHT, TRANSCRIPT_OVERSCAN } from './transcript-virtualization'

type VirtualMessageTranscriptProps = {
  messages: ChatMessage[]
  streaming?: boolean
  latestRunProps: AgentRunActivityProps
  measurementVersion: unknown
  scrollRef: RefObject<HTMLDivElement | null>
  prefixRef: RefObject<HTMLDivElement | null>
  onContentSizeChange: () => void
}

function measuredElementHeight(element: HTMLDivElement, entry?: ResizeObserverEntry) {
  const borderBox = entry?.borderBoxSize?.[0]
  return Math.ceil(borderBox?.blockSize ?? element.getBoundingClientRect().height)
}

function useTranscriptScrollMargin(
  scrollRef: RefObject<HTMLDivElement | null>,
  prefixRef: RefObject<HTMLDivElement | null>,
  listRef: RefObject<HTMLDivElement | null>,
) {
  const [scrollMargin, setScrollMargin] = useState(0)

  useLayoutEffect(() => {
    const scrollElement = scrollRef.current
    const listElement = listRef.current
    if (!scrollElement || !listElement) return undefined

    const measure = () => {
      const next = Math.max(
        0,
        listElement.getBoundingClientRect().top -
          scrollElement.getBoundingClientRect().top +
          scrollElement.scrollTop,
      )
      setScrollMargin((current) => (Math.abs(current - next) < 1 ? current : next))
    }
    measure()

    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure)
    observer?.observe(scrollElement)
    if (prefixRef.current) observer?.observe(prefixRef.current)
    window.addEventListener('resize', measure)
    return () => {
      observer?.disconnect()
      window.removeEventListener('resize', measure)
    }
  }, [listRef, prefixRef, scrollRef])

  return scrollMargin
}

export const VirtualMessageTranscript = memo(function VirtualMessageTranscript({
  messages,
  streaming,
  latestRunProps,
  measurementVersion,
  scrollRef,
  prefixRef,
  onContentSizeChange,
}: VirtualMessageTranscriptProps) {
  const listRef = useRef<HTMLDivElement>(null)
  const messagesRef = useRef(messages)
  messagesRef.current = messages
  const scrollMargin = useTranscriptScrollMargin(scrollRef, prefixRef, listRef)
  const getItemKey = useCallback(
    (index: number) => messagesRef.current[index]?.id ?? `transcript-message-${index}`,
    [],
  )
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: messages.length,
    getScrollElement: () => scrollRef.current,
    getItemKey,
    estimateSize: () => TRANSCRIPT_ESTIMATED_ROW_HEIGHT,
    measureElement: measuredElementHeight,
    overscan: TRANSCRIPT_OVERSCAN,
    scrollMargin,
    useAnimationFrameWithResizeObserver: true,
  })
  const virtualItems = virtualizer.getVirtualItems()
  const totalSize = virtualizer.getTotalSize()
  const lastMessage = messages[messages.length - 1]

  useLayoutEffect(() => {
    onContentSizeChange()
  }, [onContentSizeChange, totalSize])

  useLayoutEffect(() => {
    if (!lastMessage) return
    const element = virtualizer.elementsCache.get(lastMessage.id)
    if (element) virtualizer.measureElement(element)
  }, [lastMessage, measurementVersion, virtualizer])

  return (
    <div
      className="virtual-transcript-list"
      data-pisper-transcript-size={messages.length}
      data-pisper-rendered-count={virtualItems.length}
      ref={listRef}
      style={{ height: `${totalSize}px` }}
    >
      {virtualItems.map((virtualItem) => {
        const message = messages[virtualItem.index]
        if (!message) return null
        const isLatestAgent = message.role === 'agent' && virtualItem.index === messages.length - 1
        const agentState =
          message.streaming || (isLatestAgent && streaming)
            ? 'thinking'
            : isLatestAgent && !message.error
              ? 'waiting'
              : 'idle'
        const runProps = resolveMessageRunActivity(message, isLatestAgent, latestRunProps)
        return (
          <div
            className="virtual-transcript-item"
            data-index={virtualItem.index}
            data-pisper-virtual-item={message.id}
            key={virtualItem.key}
            ref={virtualizer.measureElement}
            style={{ top: `${virtualItem.start - scrollMargin}px` }}
          >
            <FocusChatMessage
              message={message}
              agentState={agentState}
              showRunActivity={Boolean(runProps)}
              runProps={runProps}
            />
          </div>
        )
      })}
    </div>
  )
})
