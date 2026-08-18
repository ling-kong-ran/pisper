// 虚拟化消息转录：大数据量消息列表的窗口化渲染，支持
// 顶部加载更早消息与底部自动滚动。
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type RefObject,
} from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { resolveMessageRunActivity } from '@/lib/session-state'
import type { ChatMessage } from '@/types/chat'
import type { AgentRunActivityProps } from './AgentRunActivity'
import { FocusChatMessage } from './ChatMessage'
import { TRANSCRIPT_ESTIMATED_ROW_HEIGHT, TRANSCRIPT_OVERSCAN } from './transcript-virtualization'

type VirtualMessageTranscriptProps = {
  sessionId: string
  messages: ChatMessage[]
  streaming?: boolean
  latestRunProps: AgentRunActivityProps
  measurementVersion: unknown
  scrollElement: HTMLDivElement | null
  prefixRef: RefObject<HTMLDivElement | null>
  targetEntryId?: string
  onContentSizeChange: () => void
  onTargetLocated: (entryId: string) => void
  onBranchFromHere: (boundaryEntryId: string) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
}

function measuredElementHeight(element: HTMLDivElement, entry?: ResizeObserverEntry) {
  const borderBox = entry?.borderBoxSize?.[0]
  return Math.ceil(borderBox?.blockSize ?? element.getBoundingClientRect().height)
}

function useTranscriptScrollMargin(
  scrollElement: HTMLDivElement | null,
  prefixRef: RefObject<HTMLDivElement | null>,
  listRef: RefObject<HTMLDivElement | null>,
) {
  const [scrollMargin, setScrollMargin] = useState(0)

  useLayoutEffect(() => {
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
  }, [listRef, prefixRef, scrollElement])

  return scrollMargin
}

export const VirtualMessageTranscript = memo(function VirtualMessageTranscript({
  sessionId,
  messages,
  streaming,
  latestRunProps,
  measurementVersion,
  scrollElement,
  prefixRef,
  targetEntryId,
  onContentSizeChange,
  onTargetLocated,
  onBranchFromHere,
  onCreateChildSession,
}: VirtualMessageTranscriptProps) {
  const listRef = useRef<HTMLDivElement>(null)
  const [highlightedEntryId, setHighlightedEntryId] = useState('')
  const messagesRef = useRef(messages)
  messagesRef.current = messages
  const scrollMargin = useTranscriptScrollMargin(scrollElement, prefixRef, listRef)
  const getItemKey = useCallback(
    (index: number) => messagesRef.current[index]?.id ?? `transcript-message-${index}`,
    [],
  )
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: messages.length,
    getScrollElement: () => scrollElement,
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

  useEffect(() => {
    if (!targetEntryId) return undefined
    const targetIndex = messages.findIndex(
      (message) => message.turnBoundaryEntryId === targetEntryId,
    )
    if (targetIndex < 0) return undefined
    virtualizer.scrollToIndex(targetIndex, { align: 'center' })
    setHighlightedEntryId(targetEntryId)
    const frame = window.requestAnimationFrame(() => {
      virtualizer.scrollToIndex(targetIndex, { align: 'center' })
      onTargetLocated(targetEntryId)
    })
    return () => window.cancelAnimationFrame(frame)
  }, [messages, onTargetLocated, targetEntryId, virtualizer])

  useEffect(() => {
    if (!highlightedEntryId) return undefined
    const timer = window.setTimeout(() => setHighlightedEntryId(''), 2200)
    return () => window.clearTimeout(timer)
  }, [highlightedEntryId])

  return (
    <div
      className="relative w-full"
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
            className={`virtual-transcript-item absolute left-0 [display:flow-root] w-full rounded-[var(--r-sm)] ${message.turnBoundaryEntryId === highlightedEntryId ? 'targeted [.virtual-transcript-item&]:bg-[var(--star-soft)] [.virtual-transcript-item&]:shadow-[inset_3px_0_0_var(--star-strong)]' : ''}`}
            data-index={virtualItem.index}
            data-pisper-target-entry={
              message.turnBoundaryEntryId === highlightedEntryId
                ? message.turnBoundaryEntryId
                : undefined
            }
            data-pisper-virtual-item={message.id}
            key={virtualItem.key}
            ref={virtualizer.measureElement}
            style={{ top: `${virtualItem.start - scrollMargin}px` }}
          >
            <FocusChatMessage
              sessionId={sessionId}
              message={message}
              agentState={agentState}
              showRunActivity={Boolean(runProps)}
              runProps={runProps}
              sessionStreaming={streaming}
              onBranchFromHere={onBranchFromHere}
              onCreateChildSession={onCreateChildSession}
            />
          </div>
        )
      })}
    </div>
  )
})
