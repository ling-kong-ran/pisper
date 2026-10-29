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
import { ArrowDown, GitFork, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import { useAutoScroll } from '@/hooks/useAutoScroll'
import type { ChatMessage, EntityRecord } from '@/types/chat'
import { type SessionOpenRequest } from './dock-layout'
import {
  SESSION_SELECTED_EVENT,
  SESSION_NAVIGATION_WILL_CHANGE,
  clearSessionMessageTarget,
  getSessionMessageTarget,
} from './events'
import { activityScrollVersion } from './run-activity'
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
}: FocusTranscriptProps) {
  const { t } = useI18n()
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
      if (event.currentTarget.scrollTop <= 96) void loadOlder()
    },
    [loadOlder, sessionId, transcriptLoadState],
  )

  useLayoutEffect(() => {
    // 页提交后允许继续请求；阅读位置由虚拟器原生锚定统一维护。
    olderRequestRef.current = null
  }, [messageStart])

  useEffect(() => {
    setTargetEntryId(getSessionMessageTarget(sessionId))
    const receiveTarget = (event: Event) => {
      const request = (event as CustomEvent<SessionOpenRequest>).detail
      if (request?.sessionId === sessionId && request.targetEntryId) {
        setTargetEntryId(request.targetEntryId)
      }
    }
    window.addEventListener(SESSION_SELECTED_EVENT, receiveTarget)
    return () => window.removeEventListener(SESSION_SELECTED_EVENT, receiveTarget)
  }, [sessionId])

  useEffect(() => {
    if (!targetEntryId || messageStart == null) return
    if (messages.some((message) => message.turnBoundaryEntryId === targetEntryId)) return
    if (loadingOlder) return
    if (hasOlder) {
      void loadOlder()
      return
    }
    clearSessionMessageTarget(sessionId, targetEntryId)
    setTargetEntryId('')
  }, [hasOlder, loadOlder, loadingOlder, messageStart, messages, sessionId, targetEntryId])

  useEffect(() => {
    if (scrollRequest) scrollToBottom('smooth')
  }, [scrollRequest, scrollToBottom])

  return (
    <div className="relative min-h-0 flex-1">
      <div
        className="transcript [.focus-session.has-conversation_&]:p-[var(--chat-transcript-block-padding,30px)_max(24px,calc((100%_-_var(--chat-content-width,1040px))/2))] [.focus-session.has-conversation_&]:[scroll-padding-bottom:32px] @max-[700px]:p-[20px_14px] @max-[700px]:[.focus-session.has-conversation_&]:p-[24px_16px] @max-[470px]:[padding-inline:10px] max-[650px]:p-[20px_14px] min-h-0 h-full flex-1 overflow-auto overscroll-contain scroll-auto [overflow-anchor:none] [scrollbar-gutter:stable_both-edges] m-0 border-0 [padding:26px_max(24px,calc((100%_-_var(--chat-content-width,1040px))/2))] [padding-bottom:70px] [scroll-padding-bottom:70px]"
        data-pisper-transcript-state={transcriptLoadState}
        aria-busy={transcriptLoadState === 'loading'}
        ref={setTranscriptRef}
        onScroll={handleTranscriptScroll}
        tabIndex={0}
      >
        <div className="[display:flow-root] w-full" ref={transcriptPrefixRef}>
          {lineage?.parentSessionId && (
            <div
              className="history-page-loader flex w-[min(var(--chat-content-width,1040px),100%)] min-h-[42px] items-center justify-center gap-[7px] [margin:0_auto_18px] text-[var(--text-muted)] text-[12px] session-lineage"
              data-pisper-parent-session={lineage.parentSessionId}
            >
              <GitFork size={13} />
              <span>
                {t('chat:focusSession.derivedFromSession', {
                  name: lineage.sourceSessionName || t('chat:focusSession.unknownSourceSession'),
                })}
              </span>
            </div>
          )}
          {(hasOlder || loadingOlder || olderError) && (
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
              messages={messages}
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
            />
          </div>
        )}
      </div>
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
