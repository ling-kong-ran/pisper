// 虚拟化消息转录：大数据量消息列表的窗口化渲染，支持
// 顶部加载更早消息与底部自动滚动。
import {
  memo,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from 'react'
import { useVirtualizer, type Virtualizer } from '@tanstack/react-virtual'
import { lastSettledMessageIndex, resolveMessageRunActivity } from '@/lib/session-state'
import type { ChatMessage } from '@/types/chat'
import type { AgentRunActivityProps } from './AgentRunActivity'
import { FocusChatMessage } from './ChatMessage'
import { AgentJobEvent } from './AgentJobEvent'
import { BotLinkEvent } from './BotLinkEvent'
import { AgentJobsContext } from './session-agents'
import { estimateTranscriptRowHeight, TRANSCRIPT_OVERSCAN } from './transcript-virtualization'
import type { BranchFromHereOptions } from './chat-api'
import type { ConversationAnchor } from './session-context-pages'
import { anchorScrollDelta } from './transcript-reading-position'

type VirtualMessageTranscriptProps = {
  sessionId: string
  layoutMeasurementKey?: string
  messages: ChatMessage[]
  historical?: boolean
  streaming?: boolean
  latestRunProps: AgentRunActivityProps
  scrollElement: HTMLDivElement | null
  prefixRef: RefObject<HTMLDivElement | null>
  /** 会话工作区根目录：Markdown 内相对路径文件链接的解析基址。 */
  cwd?: string
  targetEntryId?: string
  // 回到往上翻着读的会话：离开时视口顶上那条消息。只在挂载时用一次。
  restoreAnchor?: ConversationAnchor | null
  onContentSizeChange: () => void
  // 转录正贴着最新消息跟随（由 useAutoScroll 定位置）。
  isFollowingLatest?: () => boolean
  onTargetScroll: () => void
  onTargetLocated: (entryId: string) => void
  onBranchFromHere?: (
    boundaryEntryId: string,
    options?: BranchFromHereOptions,
  ) => Promise<void> | void
  onCreateChildSession?: (boundaryEntryId: string) => Promise<void> | void
  onRetryLastTurn: () => Promise<void> | void
  // 只读转录（子代理的过程）：没有重试。
  readOnly?: boolean
  /** 已加载的消息是否从会话开头开始：只有这样才数得出“第 N 轮”。 */
  turnsKnown?: boolean
  // 列表前面被收起的用户消息条数（手机会话页的“上一轮”胶囊）：第 N 轮从它之后接着数。
  turnBase?: number
}

// 读者自己动了滚动就不再校正阅读位置。
const INTERACTION_EVENTS = ['wheel', 'touchstart', 'pointerdown', 'keydown'] as const
const ANCHOR_RESTORE_MAX_FRAMES = 40

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
    let measureFrame: number | null = null
    const scheduleMeasure = () => {
      if (measureFrame !== null) return
      measureFrame = window.requestAnimationFrame(() => {
        measureFrame = null
        measure()
      })
    }
    measure()

    const observer =
      typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(scheduleMeasure)
    observer?.observe(scrollElement)
    if (prefixRef.current) observer?.observe(prefixRef.current)
    // 内容锚定底部时，列表变高会把它自己往上推：列表尺寸变化也要重测偏移。
    observer?.observe(listElement)
    window.addEventListener('resize', scheduleMeasure)
    return () => {
      observer?.disconnect()
      window.removeEventListener('resize', scheduleMeasure)
      if (measureFrame !== null) window.cancelAnimationFrame(measureFrame)
    }
  }, [listRef, prefixRef, scrollElement])

  return scrollMargin
}

export const VirtualMessageTranscript = memo(function VirtualMessageTranscript({
  sessionId,
  layoutMeasurementKey,
  messages,
  historical = false,
  streaming,
  latestRunProps,
  scrollElement,
  prefixRef,
  cwd,
  targetEntryId,
  restoreAnchor = null,
  onContentSizeChange,
  isFollowingLatest,
  onTargetScroll,
  onTargetLocated,
  onBranchFromHere,
  onCreateChildSession,
  onRetryLastTurn,
  readOnly = false,
  turnsKnown = false,
  turnBase = 0,
}: VirtualMessageTranscriptProps) {
  const agentJobs = useContext(AgentJobsContext)
  const listRef = useRef<HTMLDivElement>(null)
  const [highlightedEntryId, setHighlightedEntryId] = useState('')
  const messageIds = messages.map((message) => message.id)
  const messageKeysRef = useRef(messageIds)
  if (
    messageKeysRef.current.length !== messageIds.length ||
    messageKeysRef.current.some((id, index) => id !== messageIds[index])
  )
    messageKeysRef.current = messageIds
  const messageKeys = messageKeysRef.current
  // 每条消息记录最近的前置用户消息 ID，供最新一条助手消息上的「重试」按钮重新发送该轮输入；
  // 旧节点不提供重试——那会把历史提问追加到会话末尾，语义与「从此处继续/另开对话」冲突。
  const retryUserIdByIndex = useMemo(() => {
    const result: (string | undefined)[] = Array.from({ length: messages.length })
    let lastUserId: string | undefined
    for (let index = 0; index < messages.length; index += 1) {
      if (messages[index]?.role === 'user') lastUserId = messages[index].id
      result[index] = lastUserId
    }
    return result
  }, [messages])
  // 第 N 轮 = 截至这条消息的用户消息数；更早的历史还没加载时数不准，干脆不给。
  const turnByIndex = useMemo(() => {
    if (!turnsKnown) return null
    const result: number[] = Array.from({ length: messages.length })
    let turns = turnBase
    for (let index = 0; index < messages.length; index += 1) {
      if (messages[index]?.role === 'user') turns += 1
      result[index] = turns
    }
    return result
  }, [messages, turnBase, turnsKnown])
  const scrollMargin = useTranscriptScrollMargin(scrollElement, prefixRef, listRef)
  const getItemKey = useCallback(
    (index: number) => messageKeys[index] ?? `transcript-message-${index}`,
    [messageKeys],
  )
  // 行高估算随容器宽度分档：窄屏实际行高更大，固定估算会让滚动条明显跳动。
  // 虚拟器做行高校正（adjustments）时按它自己记的位置去挪，而那个位置只在 scroll 事件里更新，
  // 可能落后于跟随控制器刚写的位置。一次校正把位置往上拉一千多像素，跟随就被当成读者上翻而停掉，
  // 切回长会话时停在转录中间。贴底跟随时不做这种校正（跟随控制器下一帧照样定到底部）；
  // 往上翻着读时按容器此刻的实际位置平移。
  const scrollToFn = useCallback(
    (
      offset: number,
      { adjustments, behavior }: { adjustments?: number; behavior?: ScrollBehavior },
      instance: Virtualizer<HTMLDivElement, HTMLDivElement>,
    ) => {
      const element = instance.scrollElement
      if (!element) return
      if (adjustments) {
        if (isFollowingLatest?.()) return
        element.scrollTo({ top: element.scrollTop + adjustments, behavior })
        return
      }
      element.scrollTo({ top: offset, behavior })
    },
    [isFollowingLatest],
  )
  const estimateSize = useCallback(
    () => estimateTranscriptRowHeight(scrollElement?.clientWidth),
    [scrollElement],
  )
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    // ref 挂载测量发生在 React 提交期间；批处理更新避免在该阶段嵌套 flushSync。
    useFlushSync: false,
    // 原生锚定同时更新可见范围和偏移；尾页淘汰后不能用总高度差恢复前置位置。
    anchorTo: 'end',
    followOnAppend: false,
    count: messages.length,
    getScrollElement: () => scrollElement,
    scrollToFn,
    // 虚拟器第一次接上滚动容器时会把容器滚到它自己记的位置，不给就是 0：转录此前已由 useAutoScroll
    // 定到最新消息或恢复的阅读位置，被拉回顶部会触发翻页并停掉跟随。从容器当前位置读，接上时原地不动。
    initialOffset: () => scrollElement?.scrollTop ?? 0,
    getItemKey,
    estimateSize,
    measureElement: measuredElementHeight,
    overscan: TRANSCRIPT_OVERSCAN,
    scrollMargin,
    useAnimationFrameWithResizeObserver: true,
  })
  const virtualItems = virtualizer.getVirtualItems()
  const totalSize = virtualizer.getTotalSize()

  const remeasureLayout = useCallback(() => {
    const list = listRef.current
    if (!list || !scrollElement || list.getBoundingClientRect().width < 1) return
    const scrollTop = scrollElement.scrollTop
    const anchor = virtualizer.getVirtualItems().find((item) => item.end > scrollTop)
    const anchorOffset = anchor ? Math.max(0, scrollTop - anchor.start) : 0
    // ResizeObserver 只覆盖挂载行；模板与列宽改变时也要作废屏幕外的旧字号/旧宽度行高。
    virtualizer.measure()
    virtualizer.getTotalSize()
    for (const row of list.children) {
      if (row instanceof HTMLDivElement) virtualizer.measureElement(row)
    }
    virtualizer.getTotalSize()
    if (anchor) {
      const offset = virtualizer.getOffsetForIndex(anchor.index, 'start')?.[0]
      if (offset !== undefined) virtualizer.scrollToOffset(offset + anchorOffset)
    }
    onContentSizeChange()
  }, [onContentSizeChange, scrollElement, virtualizer])

  useLayoutEffect(() => {
    remeasureLayout()
  }, [layoutMeasurementKey, remeasureLayout])

  useLayoutEffect(() => {
    const list = listRef.current
    if (!list || typeof ResizeObserver === 'undefined') return
    let width = list.getBoundingClientRect().width
    let frame: number | null = null
    const observer = new ResizeObserver(() => {
      const nextWidth = list.getBoundingClientRect().width
      if (Math.abs(nextWidth - width) < 1) return
      width = nextWidth
      if (frame !== null) window.cancelAnimationFrame(frame)
      frame = window.requestAnimationFrame(() => {
        frame = null
        remeasureLayout()
      })
    })
    observer.observe(list)
    return () => {
      observer.disconnect()
      if (frame !== null) window.cancelAnimationFrame(frame)
    }
  }, [remeasureLayout])

  useLayoutEffect(() => {
    onContentSizeChange()
  }, [onContentSizeChange, totalSize])

  // 流式行高变化由虚拟器的 ResizeObserver + rAF 通道重测（useAnimationFrameWithResizeObserver），
  // 不再在 useLayoutEffect 里同步 measureElement——那会在每个流式帧强制同步布局。

  useEffect(() => {
    if (!targetEntryId) return undefined
    const targetIndex = messages.findIndex(
      (message) => message.turnBoundaryEntryId === targetEntryId,
    )
    if (targetIndex < 0) return undefined
    onTargetScroll()
    virtualizer.scrollToIndex(targetIndex, { align: 'center' })
    setHighlightedEntryId(targetEntryId)
    const frame = window.requestAnimationFrame(() => {
      virtualizer.scrollToIndex(targetIndex, { align: 'center' })
      onTargetLocated(targetEntryId)
    })
    return () => window.cancelAnimationFrame(frame)
  }, [messages, onTargetLocated, onTargetScroll, targetEntryId, virtualizer])

  // 按离开时视口顶上那条消息校正阅读位置：useAutoScroll 先按像素恢复（同时停掉跟随），但那时各行还是估算高度，
  // 像素落在别的消息上。之后几帧用真实 DOM 位置把那条消息挪回原处，稳定几帧或读者自己动了就停。
  const restoreAnchorRef = useRef(restoreAnchor)
  useEffect(() => {
    const anchor = restoreAnchorRef.current
    if (!anchor || !scrollElement) return undefined
    const index = messages.findIndex((message) => message.id === anchor.messageId)
    // 那条消息不在已加载的这一页（或收在“上一轮”胶囊里）：保留像素位置。
    if (index < 0) {
      restoreAnchorRef.current = null
      return undefined
    }
    let frame: number | null = null
    let rounds = 0
    let steady = 0
    const stop = () => {
      if (frame !== null) window.cancelAnimationFrame(frame)
      frame = null
      for (const type of INTERACTION_EVENTS) scrollElement.removeEventListener(type, finish)
    }
    // 还原完成（或读者接手）才作废锚点；effect 清理只停下，开发模式的二次挂载还能接着还原。
    const finish = () => {
      restoreAnchorRef.current = null
      stop()
    }
    const align = () => {
      frame = null
      const row = Array.from(
        listRef.current?.querySelectorAll<HTMLElement>('[data-pisper-virtual-item]') ?? [],
      ).find((node) => node.dataset.pisperVirtualItem === anchor.messageId)
      if (!row) {
        steady = 0
        virtualizer.scrollToIndex(index, { align: 'start' })
      } else {
        const delta = anchorScrollDelta(
          row.getBoundingClientRect().top,
          scrollElement.getBoundingClientRect().top,
          anchor,
        )
        if (Math.abs(delta) < 1) steady += 1
        else {
          steady = 0
          virtualizer.scrollToOffset(scrollElement.scrollTop + delta)
        }
      }
      if (steady >= 3 || ++rounds >= ANCHOR_RESTORE_MAX_FRAMES) finish()
      else frame = window.requestAnimationFrame(align)
    }
    onTargetScroll()
    for (const type of INTERACTION_EVENTS)
      scrollElement.addEventListener(type, finish, { passive: true })
    frame = window.requestAnimationFrame(align)
    return stop
    // 只在接上滚动容器时还原一次；之后的消息变化交给虚拟器自己的锚定。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scrollElement])

  useEffect(() => {
    if (!highlightedEntryId) return undefined
    const timer = window.setTimeout(() => setHighlightedEntryId(''), 2200)
    return () => window.clearTimeout(timer)
  }, [highlightedEntryId])

  const latestIndex = lastSettledMessageIndex(messages)
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
        // Bot 派出的子 agent：时间线里的一件活，不是消息。
        if (message.role === 'event') {
          const job = agentJobs?.agents.get(String(message.agentId || ''))
          const link = agentJobs?.links.get(String(message.linkId || ''))
          return (
            <div
              className="virtual-transcript-item absolute left-0 [display:flow-root] w-full"
              data-index={virtualItem.index}
              data-pisper-virtual-item={message.id}
              key={virtualItem.key}
              ref={virtualizer.measureElement}
              style={{ top: `${virtualItem.start - scrollMargin}px` }}
            >
              {job && agentJobs ? (
                <AgentJobEvent
                  sessionId={agentJobs.sessionId}
                  agent={job}
                  onChanged={agentJobs.reload}
                />
              ) : link ? (
                <BotLinkEvent link={link} />
              ) : null}
            </div>
          )
        }
        // 末尾还没送达的排队消息不算：正在写的回复仍是“最新一条”（参考 Claude，排队消息排在它后面）。
        const isLatestAgent =
          !historical && message.role === 'agent' && virtualItem.index === latestIndex
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
              cwd={cwd}
              sessionStreaming={streaming}
              onBranchFromHere={onBranchFromHere}
              onCreateChildSession={onCreateChildSession}
              retryUserMessageId={
                isLatestAgent && !readOnly ? retryUserIdByIndex[virtualItem.index] : undefined
              }
              hideErrorNotice={isLatestAgent && Boolean(message.error)}
              onRetryLastTurn={onRetryLastTurn}
              latest={isLatestAgent}
              turn={turnByIndex?.[virtualItem.index] || undefined}
            />
          </div>
        )
      })}
    </div>
  )
})
