// 聚焦转录：虚拟化的长消息流，懒加载重型子组件（如 AI 元素）。
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

import { Button } from '@/components/ui/button'

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
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
  onPromptSelect: (prompt: string) => void
  onWorkspace: () => void
}

function WelcomeFallback({ children, title }: { children: ReactNode; title: string }) {
  return (
    <div className="agent-welcome-content [:root[data-theme='light']_&]:[border:1px_solid_color-mix(in_srgb,_var(--brand-blue)_13%,_var(--stroke))] [:root[data-theme='light']_&]:rounded-[32px] [:root[data-theme='light']_&]:bg-[radial-gradient(circle_at_16%_8%,_color-mix(in_srgb,_var(--brand-blue)_10%,_transparent),_transparent_38%),_radial-gradient(circle_at_88%_92%,_rgba(139,_92,_246,_.08),_transparent_42%),_rgba(255,255,255,.82)] [:root[data-theme='light']_&]:shadow-[0_28px_72px_-48px_rgba(30,64,175,.3),_0_8px_26px_-22px_rgba(15,23,42,.22)] [:root[data-theme='light']_&]:[backdrop-filter:blur(12px)] grid w-full max-w-[640px] place-items-center [padding:28px]">
      <div className="welcome-visual [:root[data-theme='light']_&]:relative [:root[data-theme='light']_&::before]:absolute [:root[data-theme='light']_&::before]:top-[-8px] [:root[data-theme='light']_&::before]:w-[78px] [:root[data-theme='light']_&::before]:h-[78px] [:root[data-theme='light']_&::before]:[border:1px_solid_rgba(23,131,255,.12)] [:root[data-theme='light']_&::before]:rounded-[28px] [:root[data-theme='light']_&::before]:bg-[linear-gradient(145deg,_rgba(255,255,255,.94),_rgba(239,246,255,.76))] [:root[data-theme='light']_&::before]:shadow-[0_18px_42px_-26px_rgba(23,131,255,.48)] [:root[data-theme='light']_&::before]:[content:''] [:root[data-theme='light']_&::before]:[transform:rotate(8deg)] grid min-h-[144px] place-items-center">
        <BrandLogo
          size={54}
          className="welcome-logo [.agent-welcome_&]:relative [.agent-welcome_&]:z-[1] [.agent-welcome_&]:text-[var(--text)] [:root[data-theme='light']_.agent-welcome_&]:[filter:drop-shadow(0_8px_14px_rgba(23,131,255,.12))]"
        />
      </div>
      <h2>{title}</h2>
      {children}
    </div>
  )
}

function TranscriptLoading({ label }: { label: string }) {
  return (
    <div
      className="session-history-loading [&_>_strong]:text-[var(--text-soft)] [&_>_strong]:text-[13px] [&_>_strong]:font-[650] grid min-h-[100%] place-content-center justify-items-center gap-[9px] text-[var(--text-muted)] text-center [animation:transcript-stage-enter_.18s_var(--ease-out)_both]"
      role="status"
      aria-live="polite"
    >
      <div className="relative grid w-[58px] h-[58px] place-items-center" aria-hidden="true">
        <BrandLogo size={28} className="text-[var(--text)]" />
        <LoaderCircle className="absolute w-[58px] h-[58px] text-[var(--star-strong)] opacity-[.62] [animation:spin_1.1s_linear_infinite]" />
      </div>
      <strong>{label}</strong>
      <div
        className="session-history-loading-lines [&_>_i]:block [&_>_i]:w-full [&_>_i]:h-[4px] [&_>_i]:rounded-[var(--r-pill)] [&_>_i]:bg-[var(--stroke)] [&_>_i]:origin-[center] [&_>_i]:[animation:session-history-line-pulse_1.6s_ease-in-out_infinite] [&_>_i:nth-child(2)]:w-[78%] [&_>_i:nth-child(2)]:[animation-delay:.14s] [&_>_i:nth-child(3)]:w-[54%] [&_>_i:nth-child(3)]:[animation-delay:.28s] grid w-[min(360px,66vw)] justify-items-center gap-[7px] [margin-top:9px]"
        aria-hidden="true"
      >
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
  onCreateChildSession,
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
        className="welcome-workspace [&_>_svg]:flex-none [&_>_span]:flex-none [&_>_strong]:min-w-0 [&_>_strong]:overflow-hidden [&_>_strong]:text-[var(--text-soft)] [&_>_strong]:text-[12px] [&_>_strong]:text-ellipsis [&_>_small]:flex-none [&_>_small]:ml-[3px] [&_>_small]:text-[var(--star-strong)] [&_>_small]:text-[12px] [&_>_small]:font-[700] hover:bg-[var(--surface-hover)] hover:text-[var(--text)] [&:hover_>_strong]:text-[var(--text)] focus-visible:[outline:2px_solid_var(--brand-blue)] focus-visible:[outline-offset:1px] disabled:[cursor:wait] disabled:opacity-[.62] @max-[470px]:max-w-[100%] @max-[470px]:[&_>_span]:hidden flex max-w-[min(440px,100%)] min-h-[32px] items-center gap-[6px] overflow-hidden [margin-top:14px] border-0 rounded-[var(--r-xs)] bg-transparent [padding:4px_7px] text-[var(--text-muted)] text-[12px] whitespace-nowrap [transition:background_var(--d1)_var(--ease-out),_color_var(--d1)_var(--ease-out)]"
        data-target-cursor
        title={cwd}
        aria-label={t('chat:focusSession.changeWorkingDirectoryWorkspace', {
          workspace: cwd || workspaceName(cwd, language),
        })}
        onClick={onWorkspace}
        disabled={switchingCwd}
      >
        {switchingCwd ? <RefreshCw className="animate-spin" size={14} /> : <FolderOpen size={14} />}
        <span>{t('chat:focusSession.workingDirectory')}</span>
        <strong>{workspaceName(cwd, language)}</strong>
        <small>{t('chat:focusSession.changeDirectory')}</small>
      </button>
      <div className="welcome-chips [&_button]:min-h-[38px] [&_button]:[border:1px_solid_var(--stroke)] [&_button]:rounded-[var(--r-pill)] [&_button]:bg-[var(--solid)] [&_button]:p-[0_17px] [&_button]:text-[var(--text-soft)] [&_button]:text-[13px] [&_button]:font-[650] [&_button]:[transition:var(--d1)_var(--ease-out)] [&_button:hover]:border-[var(--star-border)] [&_button:hover]:bg-[var(--star-soft)] [&_button:hover]:text-[var(--star-strong)] [:root[data-theme='light']_&_button]:border-[rgba(148,163,184,.38)] [:root[data-theme='light']_&_button]:bg-[rgba(255,255,255,.88)] [:root[data-theme='light']_&_button]:shadow-[0_8px_20px_-18px_rgba(15,23,42,.45)] [:root[data-theme='light']_&_button:hover]:border-[rgba(23,131,255,.34)] [:root[data-theme='light']_&_button:hover]:bg-[#fff] [:root[data-theme='light']_&_button:hover]:text-[var(--brand-blue-strong)] [:root[data-theme='light']_&_button:hover]:shadow-[0_12px_24px_-18px_rgba(23,131,255,.55)] [:root[data-theme='light']_&_button:hover]:[transform:translateY(-1px)] flex max-w-[540px] flex-wrap justify-center gap-[9px] [margin-top:22px]">
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
        className="transcript [.focus-session.has-conversation_&]:p-[30px_max(24px,calc((100%_-_900px)/2))] [.focus-session.has-conversation_&]:[scroll-padding-bottom:32px] @max-[700px]:p-[20px_14px] @max-[700px]:[.focus-session.has-conversation_&]:p-[24px_16px] @max-[470px]:[padding-inline:10px] max-[650px]:p-[20px_14px] min-h-0 flex-1 overflow-auto m-0 border-0 [padding:26px_max(24px,calc((100%_-_960px)/2))] [scroll-padding-bottom:28px]"
        data-pisper-transcript-state={transcriptLoadState}
        aria-busy={transcriptLoadState === 'loading'}
        ref={setTranscriptRef}
        onPointerDown={cancelProgrammaticScroll}
        onScroll={handleTranscriptScroll}
        onTouchStart={cancelProgrammaticScroll}
        onWheel={cancelProgrammaticScroll}
      >
        <div className="[display:flow-root] w-full" ref={transcriptPrefixRef}>
          {lineage?.parentSessionId && (
            <div
              className="history-page-loader flex w-[min(960px,100%)] min-h-[42px] items-center justify-center gap-[7px] [margin:0_auto_18px] text-[var(--text-muted)] text-[12px] session-lineage"
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
            <div className="history-page-loader flex w-[min(960px,100%)] min-h-[42px] items-center justify-center gap-[7px] [margin:0_auto_18px] text-[var(--text-muted)] text-[12px]">
              {olderError ? (
                <Button
                  type="button"
                  variant="outline"
                  size="lg"
                  className="bg-surface-subtle"
                  onClick={loadOlder}
                >
                  <RefreshCw size={13} />
                  {t('chat:focusSession.retryOlderMessages')}
                </Button>
              ) : loadingOlder ? (
                <>
                  <RefreshCw className="animate-spin" size={14} />
                  {t('chat:focusSession.loadingOlderMessages')}
                </>
              ) : (
                <Button
                  type="button"
                  variant="outline"
                  size="lg"
                  className="bg-surface-subtle"
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
        {transcriptLoadState === 'ready' && !messages.length && (
          <div className="agent-welcome [&_h2]:mt-[16px] [&_h2]:text-[var(--text)] [&_h2]:text-[clamp(26px,_2.3vw,_30px)] [&_h2]:leading-[1.25] [&_p]:max-w-[620px] [&_p]:mt-[12px] [&_p]:text-[15px] [&_p]:leading-[1.7] [:root[data-theme='light']_&_h2]:text-[#172033] [:root[data-theme='light']_&_h2]:tracking-[-.025em] [:root[data-theme='light']_&_p]:text-[#526077] relative grid min-h-[100%] place-content-center justify-items-center overflow-hidden text-[var(--text-muted)] text-center">
            <Suspense
              fallback={<WelcomeFallback title={welcomeTitle}>{welcomeContent}</WelcomeFallback>}
            >
              <WelcomeEffects title={welcomeTitle}>{welcomeContent}</WelcomeEffects>
            </Suspense>
          </div>
        )}
        {transcriptLoadState === 'ready' && messages.length > 0 && (
          <div className="[animation:transcript-reveal-enter_.22s_var(--ease-out)_both]">
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
              onCreateChildSession={onCreateChildSession}
            />
          </div>
        )}
        {error && (
          <div className="flex w-[min(960px,100%)] items-center gap-[7px] [margin:8px_auto] rounded-[var(--r-sm)] bg-[var(--danger-soft)] text-[var(--danger)] [padding:9px_11px] text-[13px]">
            <AlertTriangle size={14} />
            {error}
          </div>
        )}
      </div>
      {hasUnread && (
        <Button
          type="button"
          variant="outline"
          size="lg"
          className="bg-surface-subtle self-center flex-none min-h-[32px] [margin:-2px_auto_6px] [border-color:var(--accent-border)] text-[var(--star-strong)] shadow-[0_8px_18px_-14px_var(--shadow)]"
          onClick={() => scrollToBottom('smooth')}
        >
          <ArrowDown size={14} />
          {t('chat:focusSession.newContent')}
        </Button>
      )}
    </>
  )
}
