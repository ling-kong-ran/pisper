// 聚焦转录：虚拟化的长消息流，懒加载重型子组件（如 AI 元素）。
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type UIEvent,
  type ReactNode,
} from 'react'
import { ArrowDown, ChevronRight, GitFork, RefreshCw } from 'lucide-react'
import { sessionPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { useAutoScroll } from '@/hooks/useAutoScroll'
import { cn } from '@/lib/utils'
import type { ChatMessage, EntityRecord } from '@/types/chat'
import { type SessionOpenRequest } from './session-open-request'
import {
  SESSION_SELECTED_EVENT,
  SESSION_NAVIGATION_WILL_CHANGE,
  clearSessionMessageTarget,
  getSessionMessageTarget,
  requestSessionSelection,
} from './events'
import { activityScrollVersion } from './run-activity'
import { earlierTurnSummary } from './transcript-turns'
import { VirtualMessageTranscript } from './VirtualMessageTranscript'
import { WorkbenchGreeting } from './WorkbenchGreeting'
import { useContextPagesStore } from './session-context-pages-store'

import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'

export type TranscriptLoadState = 'loading' | 'ready' | 'error'

type FocusTranscriptProps = {
  sessionId: string
  emptyState?: ReactNode
  messages: ChatMessage[]
  layoutMeasurementKey?: string
  transcriptLoadState?: TranscriptLoadState
  messageStart?: number | null
  hasNewer?: boolean
  onLoadLatest?: () => Promise<void> | void
  hasOlder?: boolean
  loadingOlder?: boolean
  olderError?: string
  currentActivity?: EntityRecord | null
  team?: EntityRecord | null
  activityFeed: EntityRecord[]
  tools: EntityRecord[]
  thinkingText?: string
  compaction?: EntityRecord | null
  streaming?: boolean
  runStartedAt?: string | null
  lastActivityAt?: string | null
  runFinishedAt?: string | null
  runStopped?: boolean
  runNotice?: string
  error?: string
  scrollRequest?: number
  cwd?: string
  lineage?: EntityRecord | null
  switchingCwd?: boolean
  onLoadOlder?: () => Promise<boolean> | boolean
  onBranchFromHere?: (boundaryEntryId: string) => Promise<void> | void
  onCreateChildSession?: (boundaryEntryId: string) => Promise<void> | void
  onRetryLastTurn: () => Promise<void> | void
  onPromptSelect?: (prompt: string) => void
  onWorkspace?: () => void
  // 手机会话页：只展开最新一轮，之前的对话收成“上一轮”摘要胶囊（规格 7.3），点开显示完整历史。
  collapseEarlierTurns?: boolean
}

function TranscriptLoading({ label }: { label: string }) {
  return (
    <div
      className="session-history-loading grid min-h-[100%] place-content-center justify-items-center gap-[20px] text-content-muted [animation:transcript-stage-enter_.18s_var(--ease-out)_both]"
      role="status"
      aria-live="polite"
    >
      {/* 品牌行：墨色键帽 + 轨道运行指示 + 状态文案，与转录区内的运行指示同一语言。 */}
      <div className="flex items-center gap-[10px]">
        <BrandLogo size={22} />
        <strong className="text-[13px] font-medium text-content-secondary">{label}</strong>
        <OrbitSpinner className="size-4" />
      </div>
      {/* 会话骨架：静止的细线轮廓，比起闪烁占位更安静，也能传达「对话正在成形」。 */}
      <div className="grid w-[min(560px,78vw)] gap-[12px]" aria-hidden="true">
        <div className="h-[40px] w-[46%] justify-self-end rounded-[12px_12px_3px_12px] bg-[var(--user-bubble-bg)]" />
        <div className="grid gap-[8px]">
          <div className="h-[34px] rounded-md border border-stroke-soft bg-surface-subtle" />
          <div className="h-[10px] w-[82%] rounded-full bg-surface-muted" />
          <div className="h-[10px] w-[64%] rounded-full bg-surface-muted" />
        </div>
      </div>
    </div>
  )
}

function messageIdSequence(messages: ChatMessage[]) {
  return messages.map((message) => message.id).join('\n')
}

// 父会话链接：沿用当前地址的路由形式（哈希路由时带 #），与侧栏会话行的链接一致。
function lineageHref(sessionId: string) {
  const { pathname, search, hash } = window.location
  return hash.startsWith('#/')
    ? `${pathname}${search}#${sessionPath(sessionId)}`
    : sessionPath(sessionId)
}

export function FocusTranscript({
  sessionId,
  emptyState,
  messages,
  layoutMeasurementKey,
  transcriptLoadState = 'ready',
  messageStart,
  hasOlder,
  hasNewer,
  onLoadLatest,
  loadingOlder,
  olderError,
  currentActivity,
  team,
  activityFeed,
  tools,
  thinkingText,
  compaction,
  streaming,
  runStartedAt,
  lastActivityAt,
  runFinishedAt,
  runStopped,
  runNotice,
  error,
  scrollRequest,
  cwd,
  lineage,
  onLoadOlder,
  onBranchFromHere,
  onCreateChildSession,
  onRetryLastTurn,
  collapseEarlierTurns = false,
}: FocusTranscriptProps) {
  const { t } = useI18n()
  const [earlierOpen, setEarlierOpen] = useState(false)
  const olderRequestRef = useRef<object | null>(null)
  const [loadingLatest, setLoadingLatest] = useState(false)
  const activeSessionRef = useRef(sessionId)
  activeSessionRef.current = sessionId
  useEffect(() => {
    olderRequestRef.current = null
    setLoadingLatest(false)
  }, [sessionId])
  const transcriptPrefixRef = useRef<HTMLDivElement>(null)
  const [targetEntryId, setTargetEntryId] = useState(() => getSessionMessageTarget(sessionId))
  const earlier = useMemo(
    () => (collapseEarlierTurns ? earlierTurnSummary(messages, { turnsKnown: !hasOlder }) : null),
    [collapseEarlierTurns, hasOlder, messages],
  )
  // 要跳到某一轮（例如从标签打开）时不收起，否则目标可能落在胶囊里。
  const collapsed = Boolean(earlier) && !earlierOpen && !targetEntryId
  const shownMessages = collapsed && earlier ? messages.slice(earlier.latestIndex) : messages
  const lastMessage = messages[messages.length - 1]
  const textScrollBucket = Math.floor((lastMessage?.text?.length || 0) / 64)
  const activityVersion = activityScrollVersion(activityFeed)
  const thinkingScrollBucket = Math.floor(String(thinkingText || '').length / 128)
  const transcriptVersion = `${sessionId}:${lastMessage?.id || ''}:${textScrollBucket}:${thinkingScrollBucket}:${lastMessage?.attachments?.length || 0}:${activityVersion}:${compaction?.status || ''}:${compaction?.finishedAt || ''}:${error || ''}:${streaming ? '1' : '0'}`
  const leaving = useRef(false)
  // 恢复值在实例挂载时固定，避免初始行高测量触发 scroll 后反过来覆盖恢复目标。
  const initialReadingPosition = useRef(
    useContextPagesStore.getState().sessions[sessionId]?.conversationScrollTop ?? null,
  )
  const {
    scrollRef: transcriptRef,
    scrollElement: transcriptElement,
    setScrollRef: setTranscriptRef,
    hasUnread,
    scrollToBottom,
    maintainBottom,
    pauseFollowing,
  } = useAutoScroll(transcriptVersion, {
    resetKey: `${sessionId}:${transcriptLoadState}`,
    readInitialPosition: () =>
      transcriptLoadState === 'ready' ? initialReadingPosition.current : null,
  })
  useEffect(() => {
    let frame: number | null = null
    const save = () => {
      const element = transcriptElement
      if (!element || transcriptLoadState !== 'ready' || element.clientHeight <= 0) return
      const store = useContextPagesStore.getState()
      if (store.sessions[sessionId]?.conversationScrollTop !== element.scrollTop)
        store.update(sessionId, 'files', {
          type: 'workspace',
          conversationScrollTop: element.scrollTop,
        })
    }
    const beforeSelection = () => {
      save()
      leaving.current = true
      if (frame !== null) cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => {
        leaving.current = false
        frame = null
      })
    }
    // Dock 会先移除 DOM 再卸载 React；使用显式导航前通知保存；选中事件与卸载发生时尺寸可能已为零。
    window.addEventListener(SESSION_NAVIGATION_WILL_CHANGE, beforeSelection)
    window.addEventListener('pointerdown', save, true)
    return () => {
      window.removeEventListener(SESSION_NAVIGATION_WILL_CHANGE, beforeSelection)
      if (frame !== null) cancelAnimationFrame(frame)
      window.removeEventListener('pointerdown', save, true)
    }
  }, [sessionId, transcriptElement, transcriptLoadState])
  const latestRunProps = useMemo(
    () => ({
      streaming,
      text: lastMessage?.role === 'agent' ? lastMessage.text : '',
      currentActivity,
      team,
      activityFeed,
      tools,
      thinkingText,
      compaction,
      error: error || (lastMessage?.role === 'agent' ? lastMessage.error : ''),
      stopped: runStopped,
      notice: runNotice,
      startedAt: runStartedAt,
      lastActivityAt,
      finishedAt: runFinishedAt,
    }),
    [
      streaming,
      lastMessage,
      currentActivity,
      team,
      activityFeed,
      tools,
      thinkingText,
      compaction,
      error,
      runStopped,
      runNotice,
      runStartedAt,
      lastActivityAt,
      runFinishedAt,
    ],
  )
  const loadOlder = useCallback(async () => {
    const node = transcriptRef.current
    if (!node || !hasOlder || loadingOlder || olderRequestRef.current) return
    pauseFollowing()
    const identity = {}
    olderRequestRef.current = identity
    const originSession = sessionId
    const loaded = await onLoadOlder?.()
    if (activeSessionRef.current !== originSession) return
    if (!loaded && olderRequestRef.current === identity) olderRequestRef.current = null
  }, [hasOlder, loadingOlder, onLoadOlder, pauseFollowing, sessionId, transcriptRef])
  const handleTranscriptScroll = useCallback(
    (event: UIEvent<HTMLDivElement>) => {
      if (
        !leaving.current &&
        transcriptLoadState === 'ready' &&
        event.currentTarget.clientHeight > 0
      ) {
        useContextPagesStore.getState().update(sessionId, 'files', {
          type: 'workspace',
          conversationScrollTop: event.currentTarget.scrollTop,
        })
      }
      // 收起更早的对话时不自动翻页：胶囊之上没有可读内容，翻页只会白白拉历史。
      if (!collapsed && event.currentTarget.scrollTop <= 96) void loadOlder()
    },
    [collapsed, loadOlder, sessionId, transcriptLoadState],
  )

  useLayoutEffect(() => {
    // 页提交后允许继续请求；阅读位置由虚拟器原生锚定统一维护。
    olderRequestRef.current = null
  }, [messageStart])

  // 带定位目标的选中请求会让聊天页强制重读消息（切到另一条分支后，目标只在重读结果里）。
  // 重读结果到来之前不判定“找不到”，否则目标在旧分支的消息里找不到就被提前清掉，也不会去翻旧分支的更早消息。
  // 重读迟迟不来（或结果与原来同一份）时 4 秒后照常判定。按消息 id 序列比较：同一份消息在重读期间也会换数组。
  const latestMessagesRef = useRef(messages)
  useEffect(() => {
    latestMessagesRef.current = messages
  })
  const targetSnapshotRef = useRef<string | null>(null)
  const [targetGraceEnded, setTargetGraceEnded] = useState(0)
  useEffect(() => {
    setTargetEntryId(getSessionMessageTarget(sessionId))
    let graceTimer = 0
    const receiveTarget = (event: Event) => {
      const request = (event as CustomEvent<SessionOpenRequest>).detail
      if (request?.sessionId === sessionId && request.targetEntryId) {
        targetSnapshotRef.current = messageIdSequence(latestMessagesRef.current)
        window.clearTimeout(graceTimer)
        graceTimer = window.setTimeout(() => {
          targetSnapshotRef.current = null
          setTargetGraceEnded((value) => value + 1)
        }, 4000)
        setTargetEntryId(request.targetEntryId)
      }
    }
    window.addEventListener(SESSION_SELECTED_EVENT, receiveTarget)
    return () => {
      window.clearTimeout(graceTimer)
      targetSnapshotRef.current = null
      window.removeEventListener(SESSION_SELECTED_EVENT, receiveTarget)
    }
  }, [sessionId])

  useEffect(() => {
    if (!targetEntryId || messageStart == null) return
    if (messages.some((message) => message.turnBoundaryEntryId === targetEntryId)) return
    if (targetSnapshotRef.current !== null) {
      if (targetSnapshotRef.current === messageIdSequence(messages)) return
      targetSnapshotRef.current = null
    }
    if (loadingOlder) return
    if (hasOlder) {
      void loadOlder()
      return
    }
    clearSessionMessageTarget(sessionId, targetEntryId)
    setTargetEntryId('')
  }, [
    hasOlder,
    loadOlder,
    loadingOlder,
    messageStart,
    messages,
    sessionId,
    targetEntryId,
    targetGraceEnded,
  ])

  useEffect(() => {
    if (scrollRequest) scrollToBottom('smooth')
  }, [scrollRequest, scrollToBottom])

  return (
    <div className="relative min-h-0 flex-1">
      <div
        className="transcript flex flex-col [.focus-session.has-conversation_&]:p-[var(--chat-transcript-block-padding,30px)_max(24px,calc((100%_-_var(--chat-content-width,1040px))/2))] [.focus-session.has-conversation_&]:[scroll-padding-bottom:32px] @max-[700px]:p-[20px_14px] @max-[700px]:[.focus-session.has-conversation_&]:p-[24px_16px] @max-[470px]:[padding-inline:10px] max-[650px]:p-[20px_14px] min-h-0 h-full flex-1 overflow-auto overscroll-contain scroll-auto [overflow-anchor:none] [scrollbar-gutter:stable_both-edges] m-0 border-0 [padding:26px_max(24px,calc((100%_-_var(--chat-content-width,1040px))/2))] [padding-bottom:70px] [scroll-padding-bottom:70px]"
        data-pisper-transcript-state={transcriptLoadState}
        aria-busy={transcriptLoadState === 'loading'}
        ref={setTranscriptRef}
        onScroll={handleTranscriptScroll}
        tabIndex={0}
      >
        {/* 内容锚定底部（规格 4.3）：消息不满一屏时整段贴着输入框，而不是悬在顶部。 */}
        <div
          className={`[display:flow-root] w-full ${transcriptLoadState === 'ready' && messages.length > 0 ? 'mt-auto' : ''}`}
          ref={transcriptPrefixRef}
        >
          {typeof lineage?.parentSessionId === 'string' && lineage.parentSessionId && (
            // “派生自”横幅是链接（老问题 5）：打开父会话并定位到派生点那一回合；只打开，不导航父会话的树。
            // 带修饰键或中键时交给浏览器（新标签打开、复制链接照常可用）。
            <div className="history-page-loader flex w-[min(var(--chat-content-width,1040px),100%)] min-h-[42px] items-center justify-center [margin:0_auto_18px] text-[var(--text-muted)] text-[12px] session-lineage">
              <a
                href={lineageHref(lineage.parentSessionId)}
                className="inline-flex min-h-8 max-w-full items-center gap-[7px] rounded-md px-2 text-inherit underline-offset-2 transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content hover:underline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus max-[650px]:min-h-11"
                data-pisper-parent-session={lineage.parentSessionId}
                title={t('chat:focusSession.openSourceTurn')}
                onClick={(event) => {
                  if (
                    event.button !== 0 ||
                    event.metaKey ||
                    event.ctrlKey ||
                    event.shiftKey ||
                    event.altKey
                  )
                    return
                  event.preventDefault()
                  requestSessionSelection(
                    String(lineage.parentSessionId),
                    'open',
                    typeof lineage.sourceEntryId === 'string' ? lineage.sourceEntryId : '',
                  )
                }}
              >
                <GitFork size={13} aria-hidden="true" className="flex-none" />
                <span className="min-w-0 truncate">
                  {t('chat:focusSession.derivedFromSession', {
                    name: lineage.sourceSessionName || t('chat:focusSession.unknownSourceSession'),
                  })}
                </span>
              </a>
            </div>
          )}
          {collapsed && earlier && (
            <div className="flex justify-center pb-3">
              <button
                type="button"
                data-earlier-turns={earlier.turnBase}
                aria-expanded={false}
                aria-label={t('board:phone.earlierTurns', {
                  summary: [
                    earlier.previous.turn
                      ? t('board:transcript.turn', { count: earlier.previous.turn })
                      : t('board:phone.previousTurn'),
                    earlier.previous.text,
                  ]
                    .filter(Boolean)
                    .join(' · '),
                })}
                className="inline-flex min-h-11 max-w-full min-w-0 items-center gap-1.5 rounded-full px-3 text-xs text-content-muted ring-1 ring-stroke ring-inset transition-colors duration-(--d-fast) hover:bg-surface-hover focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus"
                onClick={() => setEarlierOpen(true)}
              >
                <StatusGlyph
                  status={earlier.previous.failed ? 'failed' : 'done'}
                  className={cn(
                    'size-3.5 flex-none',
                    earlier.previous.failed ? 'text-st-fail' : 'text-st-done',
                  )}
                />
                <span className="min-w-0 truncate">
                  {[
                    earlier.previous.turn
                      ? t('board:transcript.turn', { count: earlier.previous.turn })
                      : t('board:phone.previousTurn'),
                    earlier.previous.text,
                    earlier.previous.operations
                      ? t('board:transcript.operations', { count: earlier.previous.operations })
                      : '',
                  ]
                    .filter(Boolean)
                    .join(' · ')}
                </span>
                <ChevronRight className="size-3.5 flex-none" aria-hidden="true" />
              </button>
            </div>
          )}
          {!collapsed && (hasOlder || loadingOlder || olderError) && (
            <div className="history-page-loader flex w-[min(var(--chat-content-width,1040px),100%)] min-h-[42px] items-center justify-center gap-[7px] [margin:0_auto_18px] text-[var(--text-muted)] text-[12px]">
              {olderError ? (
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  className="max-[650px]:h-11"
                  onClick={loadOlder}
                >
                  <RefreshCw size={13} />
                  {t('chat:focusSession.retryOlderMessages')}
                </Button>
              ) : loadingOlder ? (
                <>
                  <OrbitSpinner className="size-3.5" />
                  {t('chat:focusSession.loadingOlderMessages')}
                </>
              ) : (
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  className="max-[650px]:h-11"
                  onClick={loadOlder}
                >
                  <ArrowDown
                    className="history-up-arrow [.history-page-loader_&]:[transform:rotate(180deg)]"
                    size={14}
                  />
                  {t('chat:focusSession.loadOlderMessages')}
                </Button>
              )}
            </div>
          )}
        </div>
        {transcriptLoadState === 'loading' && (
          <TranscriptLoading label={t('chat:focusSession.loadingConversationHistory')} />
        )}
        {transcriptLoadState === 'ready' &&
          !messages.length &&
          (emptyState ?? (
            <div className="agent-welcome relative grid min-h-full place-content-center justify-items-center text-center text-muted-foreground [[data-mobile-keyboard='open']_&]:pointer-events-none [[data-mobile-keyboard-transition='opening']_&]:pointer-events-none [[data-mobile-keyboard-transition='closing']_&]:pointer-events-none">
              <WorkbenchGreeting />
            </div>
          ))}
        {transcriptLoadState === 'ready' && messages.length > 0 && (
          <div className="[animation:transcript-reveal-enter_.22s_var(--ease-out)_both]">
            <VirtualMessageTranscript
              key={sessionId}
              layoutMeasurementKey={layoutMeasurementKey}
              sessionId={sessionId}
              messages={shownMessages}
              historical={hasNewer}
              streaming={streaming}
              latestRunProps={latestRunProps}
              scrollElement={transcriptElement}
              prefixRef={transcriptPrefixRef}
              cwd={cwd}
              targetEntryId={targetEntryId}
              onContentSizeChange={maintainBottom}
              onTargetScroll={pauseFollowing}
              onTargetLocated={(entryId) => {
                clearSessionMessageTarget(sessionId, entryId)
                setTargetEntryId('')
              }}
              onBranchFromHere={onBranchFromHere}
              onCreateChildSession={onCreateChildSession}
              onRetryLastTurn={onRetryLastTurn}
              turnsKnown={!hasOlder}
              turnBase={collapsed && earlier ? earlier.turnBase : 0}
            />
          </div>
        )}
      </div>
      {transcriptLoadState === 'ready' && messages.length > 0 && (
        // 顶部渐隐：滚动中的内容在页头下方柔和淡出，不与页头细线硬切。手机代理条下方没有细线分隔，
        // 24px 只能盖住半行信息行，加高到 40px。
        <div
          aria-hidden="true"
          className="pointer-events-none absolute inset-x-0 top-0 z-[2] h-6 bg-linear-to-b from-panel to-transparent max-[650px]:h-10"
        />
      )}
      {(hasUnread || hasNewer) && (
        <div className="pointer-events-none absolute inset-x-0 bottom-[6px] z-[5] flex justify-center">
          <Button
            type="button"
            variant="secondary"
            size="sm"
            className="pointer-events-auto rounded-full px-3 shadow-float max-[650px]:h-11"
            disabled={loadingLatest}
            aria-busy={loadingLatest || undefined}
            onClick={async () => {
              if (!hasNewer) {
                scrollToBottom('smooth')
                return
              }
              const originSession = sessionId
              if (hasNewer) {
                setLoadingLatest(true)
                try {
                  await onLoadLatest?.()
                } finally {
                  if (activeSessionRef.current === originSession) setLoadingLatest(false)
                }
              }
              if (activeSessionRef.current !== originSession) return
              requestAnimationFrame(() => scrollToBottom('auto'))
            }}
          >
            <ArrowDown size={14} />
            {hasNewer ? t('chat:focusSession.returnToLatest') : t('chat:focusSession.newContent')}
          </Button>
        </div>
      )}
    </div>
  )
}
