import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, type UIEvent } from 'react'
import { AlertTriangle, ArrowDown, RefreshCw } from 'lucide-react'
import type { I18nValues } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import { AsciiText, Aurora, BlurText, TargetCursor } from '@/components/react-bits'
import { useAutoScroll } from '@/hooks/useAutoScroll'
import type { ChatMessage, EntityRecord, Plan } from '@/types/chat'
import PlanBoard from './PlanBoard'
import { activityScrollVersion } from './run-activity'
import {
  anchoredScrollTopAfterPrepend,
  type TranscriptPrependSnapshot,
} from './transcript-virtualization'
import { VirtualMessageTranscript } from './VirtualMessageTranscript'

type Translate = (message: string, values?: I18nValues) => string

type FocusTranscriptProps = {
  sessionId: string
  messages: ChatMessage[]
  messageStart?: number | null
  hasOlder?: boolean
  loadingOlder?: boolean
  olderError?: string
  plan?: Plan | null
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
  onLoadOlder?: () => Promise<boolean> | boolean
  onPromptSelect: (prompt: string) => void
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
  ]
}

export function FocusTranscript({
  sessionId,
  messages,
  messageStart,
  hasOlder,
  loadingOlder,
  olderError,
  plan,
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
  onLoadOlder,
  onPromptSelect,
}: FocusTranscriptProps) {
  const { t } = useI18n()
  const prependSnapshot = useRef<TranscriptPrependSnapshot | null>(null)
  const transcriptPrefixRef = useRef<HTMLDivElement>(null)
  const lastMessage = messages[messages.length - 1]
  const textScrollBucket = Math.floor((lastMessage?.text?.length || 0) / 64)
  const activityVersion = activityScrollVersion(activityFeed)
  const thinkingScrollBucket = Math.floor(String(thinkingText || '').length / 128)
  const transcriptVersion = `${sessionId}:${lastMessage?.id || ''}:${textScrollBucket}:${thinkingScrollBucket}:${lastMessage?.attachments?.length || 0}:${activityVersion}:${plan?.updatedAt || ''}:${compaction?.status || ''}:${compaction?.finishedAt || ''}:${error || ''}:${streaming ? '1' : '0'}`
  const {
    scrollRef: transcriptRef,
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
    if (scrollRequest) scrollToBottom('smooth')
  }, [scrollRequest, scrollToBottom])

  return (
    <>
      <div
        className="transcript"
        ref={transcriptRef}
        onPointerDown={cancelProgrammaticScroll}
        onScroll={handleTranscriptScroll}
        onTouchStart={cancelProgrammaticScroll}
        onWheel={cancelProgrammaticScroll}
      >
        <div className="transcript-prefix" ref={transcriptPrefixRef}>
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
          {plan?.items?.length ? <PlanBoard plan={plan} /> : null}
        </div>
        {!messages.length && (
          <div className="agent-welcome">
            <Aurora />
            <TargetCursor className="agent-welcome-content">
              <div className="welcome-visual">
                <BrandLogo size={54} className="welcome-logo" />
                <AsciiText text="PISPER" />
              </div>
              <h2>
                <BlurText text={t('chat:focusSession.letSBeginWithASparkOfAnIdea')} />
              </h2>
              <p>
                {t(
                  'chat:focusSession.pisperIsReadyToReadTheCurrentWorkspaceSearchTheCodebaseAndHelpCarryTheTaskThroughItRunsInTheWork',
                )}
              </p>
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
            </TargetCursor>
          </div>
        )}
        {messages.length > 0 && (
          <VirtualMessageTranscript
            key={sessionId}
            messages={messages}
            streaming={streaming}
            latestRunProps={latestRunProps}
            measurementVersion={transcriptVersion}
            scrollRef={transcriptRef}
            prefixRef={transcriptPrefixRef}
            onContentSizeChange={maintainBottom}
          />
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
