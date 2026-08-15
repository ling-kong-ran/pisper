import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type UIEvent,
} from 'react'
import {
  AlertTriangle,
  ArrowDown,
  FolderOpen,
  GitFork,
  LoaderCircle,
  RefreshCw,
} from 'lucide-react'
import type { I18nValues } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import { useAutoScroll } from '@/hooks/useAutoScroll'
import { workspaceName } from '@/lib/format'
import type { ChatMessage, EntityRecord } from '@/types/chat'
import { type SessionOpenRequest } from './dock-layout'
import {
  SESSION_SELECTED_EVENT,
  clearSessionMessageTarget,
  getSessionMessageTarget,
} from './events'
import { activityScrollVersion } from './run-activity'
import {
  anchoredScrollTopAfterPrepend,
  type TranscriptPrependSnapshot,
} from './transcript-virtualization'
import { VirtualMessageTranscript } from './VirtualMessageTranscript'

const WelcomeEffects = lazy(() => import('./WelcomeEffects'))

type Translate = (message: string, values?: I18nValues) => string

export type TranscriptLoadState = 'loading' | 'ready' | 'error'

type FocusTranscriptProps = {
  sessionId: string
  messages: ChatMessage[]
  transcriptLoadState?: TranscriptLoadState
  messageStart?: number | null
  hasOlder?: boolean
  loadingOlder?: boolean
  olderError?: string
  currentActivity?: EntityRecord | null
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
  onBranchFromHere: (boundaryEntryId: string) => Promise<void> | void
  onPromptSelect: (prompt: string) => void
  onWorkspace: () => void
}

function WelcomeFallback({ children, title }: { children: ReactNode; title: string }) {
  return (
    <div className="agent-welcome-content">
      <div className="welcome-visual">
        <BrandLogo size={54} className="welcome-logo" />
      </div>
      <h2>{title}</h2>
      {children}
    </div>
  )
}

function TranscriptLoading({ label }: { label: string }) {
  return (
    <div className="session-history-loading" role="status" aria-live="polite">
      <div className="session-history-loading-mark" aria-hidden="true">
        <BrandLogo size={28} className="session-history-loading-logo" />
        <LoaderCircle className="session-history-loading-ring" />
      </div>
      <strong>{label}</strong>
      <div className="session-history-loading-lines" aria-hidden="true">
        <i />
        <i />
        <i />
      </div>
    </div>
  )
}

function welcomeChips(t: Translate) {
  return [
    {
      label: t('chat:focusSession.explainCode'),
      prompt: t('chat:focusSession.explainHowThisCodeWorks'),
    },
    {
      label: t('chat:focusSession.writeTests'),
      prompt: t('chat:focusSession.writeUnitTestsForTheFollowingCode'),
    },
    {
      label: t('chat:focusSession.refactor'),
      prompt: t('chat:focusSession.refactorThisCodeAndExplainTheImprovements'),
    },
    {
      label: t('chat:focusSession.findABug'),
      prompt: t('chat:focusSession.helpMeLocateAndFixThisBug'),
    },
    {
      label: t('chat:focusSession.summarize'),
      prompt: t('chat:focusSession.summarizeContentAndExtractKeyPoints'),
    },
    {
      label: t('chat:focusSession.makeAPlan'),
      prompt: t('chat:focusSession.makeAClearActionablePlanForThisGoal'),
    },
    {
      label: t('chat:focusSession.organizeInformation'),
      prompt: t('chat:focusSession.organizeThisInformationIntoAClearStructure'),
    },
    {
      label: t('chat:focusSession.findIssues'),
      prompt: t('chat:focusSession.reviewThisContentAndSuggestImprovements'),
    },
  ]
}

export function FocusTranscript({
  sessionId,
  messages,
  transcriptLoadState = 'ready',
  messageStart,
  hasOlder,
  loadingOlder,
  olderError,
  currentActivity,
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
  switchingCwd,
  onLoadOlder,
  onBranchFromHere,
  onPromptSelect,
  onWorkspace,
}: FocusTranscriptProps) {
  const { t, language } = useI18n()
  const prependSnapshot = useRef<TranscriptPrependSnapshot | null>(null)
  const transcriptPrefixRef = useRef<HTMLDivElement>(null)
  const [targetEntryId, setTargetEntryId] = useState(() => getSessionMessageTarget(sessionId))
  const lastMessage = messages[messages.length - 1]
  const textScrollBucket = Math.floor((lastMessage?.text?.length || 0) / 64)
  const activityVersion = activityScrollVersion(activityFeed)
  const thinkingScrollBucket = Math.floor(String(thinkingText || '').length / 128)
  const transcriptVersion = `${sessionId}:${lastMessage?.id || ''}:${textScrollBucket}:${thinkingScrollBucket}:${lastMessage?.attachments?.length || 0}:${activityVersion}:${compaction?.status || ''}:${compaction?.finishedAt || ''}:${error || ''}:${streaming ? '1' : '0'}`
  const {
    scrollRef: transcriptRef,
    scrollElement: transcriptElement,
    setScrollRef: setTranscriptRef,
    onScroll: onTranscriptScroll,
    hasUnread,
    scrollToBottom,
    maintainBottom,
    cancelProgrammaticScroll,
  } = useAutoScroll(transcriptVersion)
  const latestRunProps = useMemo(
    () => ({
      streaming,
      text: lastMessage?.role === 'agent' ? lastMessage.text : '',
      currentActivity,
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
    if (!node || !hasOlder || loadingOlder || prependSnapshot.current) return
    prependSnapshot.current = { scrollHeight: node.scrollHeight, scrollTop: node.scrollTop }
    const loaded = await onLoadOlder?.()
    if (!loaded) prependSnapshot.current = null
  }, [hasOlder, loadingOlder, onLoadOlder, transcriptRef])
  const handleTranscriptScroll = useCallback(
    (event: UIEvent<HTMLDivElement>) => {
      onTranscriptScroll(event)
      if (event.currentTarget.scrollTop <= 96) void loadOlder()
    },
    [loadOlder, onTranscriptScroll],
  )

  useLayoutEffect(() => {
    const snapshot = prependSnapshot.current
    const node = transcriptRef.current
    if (!snapshot || !node) return
    node.scrollTop = anchoredScrollTopAfterPrepend(snapshot, node.scrollHeight)
    prependSnapshot.current = null
  }, [messageStart, transcriptRef])

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

  const welcomeTitle = t('chat:focusSession.letSBeginWithASparkOfAnIdea')
  const welcomeContent = (
    <>
      <p>{t('chat:focusSession.readyToWorkWithTheCurrentDirectoryAndHelpCompleteTheTask')}</p>
      <button
        type="button"
        className="welcome-workspace"
        data-target-cursor
        title={cwd}
        aria-label={t('chat:focusSession.changeWorkingDirectoryWorkspace', {
          workspace: cwd || workspaceName(cwd, language),
        })}
        onClick={onWorkspace}
        disabled={switchingCwd}
      >
        {switchingCwd ? <RefreshCw className="spin" size={14} /> : <FolderOpen size={14} />}
        <span>{t('chat:focusSession.workingDirectory')}</span>
        <strong>{workspaceName(cwd, language)}</strong>
        <small>{t('chat:focusSession.changeDirectory')}</small>
      </button>
      <div className="welcome-chips">
        {welcomeChips(t).map((chip) => (
          <button
            type="button"
            key={chip.label}
            data-target-cursor
            onClick={() => onPromptSelect(chip.prompt)}
          >
            {chip.label}
          </button>
        ))}
      </div>
    </>
  )

  return (
    <>
      <div
        className="transcript"
        data-pisper-transcript-state={transcriptLoadState}
        aria-busy={transcriptLoadState === 'loading'}
        ref={setTranscriptRef}
        onPointerDown={cancelProgrammaticScroll}
        onScroll={handleTranscriptScroll}
        onTouchStart={cancelProgrammaticScroll}
        onWheel={cancelProgrammaticScroll}
      >
        <div className="transcript-prefix" ref={transcriptPrefixRef}>
          {lineage?.parentSessionId && (
            <div
              className="history-page-loader session-lineage"
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
            <div className="history-page-loader">
              {olderError ? (
                <button type="button" className="button secondary" onClick={loadOlder}>
                  <RefreshCw size={13} />
                  {t('chat:focusSession.retryOlderMessages')}
                </button>
              ) : loadingOlder ? (
                <>
                  <RefreshCw className="spin" size={14} />
                  {t('chat:focusSession.loadingOlderMessages')}
                </>
              ) : (
                <button type="button" className="button secondary" onClick={loadOlder}>
                  <ArrowDown className="history-up-arrow" size={14} />
                  {t('chat:focusSession.loadOlderMessages')}
                </button>
              )}
            </div>
          )}
        </div>
        {transcriptLoadState === 'loading' && (
          <TranscriptLoading label={t('chat:focusSession.loadingConversationHistory')} />
        )}
        {transcriptLoadState === 'ready' && !messages.length && (
          <div className="agent-welcome">
            <Suspense
              fallback={<WelcomeFallback title={welcomeTitle}>{welcomeContent}</WelcomeFallback>}
            >
              <WelcomeEffects title={welcomeTitle}>{welcomeContent}</WelcomeEffects>
            </Suspense>
          </div>
        )}
        {transcriptLoadState === 'ready' && messages.length > 0 && (
          <div className="transcript-reveal">
            <VirtualMessageTranscript
              key={sessionId}
              sessionId={sessionId}
              messages={messages}
              streaming={streaming}
              latestRunProps={latestRunProps}
              measurementVersion={transcriptVersion}
              scrollElement={transcriptElement}
              prefixRef={transcriptPrefixRef}
              targetEntryId={targetEntryId}
              onContentSizeChange={maintainBottom}
              onTargetLocated={(entryId) => {
                clearSessionMessageTarget(sessionId, entryId)
                setTargetEntryId('')
              }}
              onBranchFromHere={onBranchFromHere}
            />
          </div>
        )}
        {error && (
          <div className="chat-error">
            <AlertTriangle size={14} />
            {error}
          </div>
        )}
      </div>
      {hasUnread && (
        <button
          type="button"
          className="button secondary jump-to-latest"
          onClick={() => scrollToBottom('smooth')}
        >
          <ArrowDown size={14} />
          {t('chat:focusSession.newContent')}
        </button>
      )}
    </>
  )
}
