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
  ArrowDown,
  Bug,
  Code2,
  FileText,
  FlaskConical,
  GitFork,
  Layers,
  ListChecks,
  RefreshCw,
  SearchCheck,
  Wand2,
} from 'lucide-react'
import type { I18nValues } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import { useAutoScroll } from '@/hooks/useAutoScroll'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import type { ChatMessage, EntityRecord } from '@/types/chat'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
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
import { WelcomeBrandStage } from './welcome-brand'

import { Button } from '@/components/ui/button'

const WelcomeEffects = lazy(() => import('./WelcomeEffects'))

type Translate = (message: string, values?: I18nValues) => string

export type TranscriptLoadState = 'loading' | 'ready' | 'error'

type FocusTranscriptProps = {
  sessionId: string
  messages: ChatMessage[]
  layoutMeasurementKey?: string
  transcriptLoadState?: TranscriptLoadState
  messageStart?: number | null
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
  onBranchFromHere: (boundaryEntryId: string) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
  onRetryLastTurn: () => Promise<void> | void
  onPromptSelect: (prompt: string) => void
  onWorkspace: () => void
}

function WelcomeFallback({ children, title }: { children: ReactNode; title: string }) {
  return (
    <div className="agent-welcome-content grid w-full max-w-[680px] justify-items-center gap-5 px-4 py-6">
      <WelcomeBrandStage />
      <h2 className="text-[clamp(22px,2.4vw,30px)] font-medium tracking-tight text-foreground">
        {title}
      </h2>
      {children}
    </div>
  )
}

function TranscriptLoading({ label }: { label: string }) {
  return (
    <div
      className="session-history-loading grid min-h-[100%] place-content-center justify-items-center gap-[24px] text-[var(--text-muted)] [animation:transcript-stage-enter_.18s_var(--ease-out)_both]"
      role="status"
      aria-live="polite"
    >
      {/* 品牌行:Logo + 状态文案 + 思考点 */}
      <div className="flex items-center gap-[10px]">
        <span className="grid h-[34px] w-[34px] place-items-center rounded-[11px] border border-[color-mix(in_srgb,#A855F7_24%,var(--stroke))] bg-[var(--solid)] shadow-[var(--sh-1)]">
          <BrandLogo size={19} />
        </span>
        <strong className="text-[13px] font-medium text-[var(--text-soft)]">{label}</strong>
        <span className="flex items-end gap-[3px] pb-[3px]" aria-hidden="true">
          <i className="block h-[4px] w-[4px] rounded-full bg-[#A855F7] [animation:agent-thinking-dot_1.2s_ease-in-out_infinite]" />
          <i className="block h-[4px] w-[4px] rounded-full bg-[#A855F7] [animation:agent-thinking-dot_1.2s_ease-in-out_.16s_infinite]" />
          <i className="block h-[4px] w-[4px] rounded-full bg-[#A855F7] [animation:agent-thinking-dot_1.2s_ease-in-out_.32s_infinite]" />
        </span>
      </div>
      {/* 会话骨架:比起三条进度线,消息泡形状更能传达「对话正在成形」 */}
      <div className="grid w-[min(560px,78vw)] gap-[12px]" aria-hidden="true">
        <div className="h-[52px] w-[68%] rounded-[16px_16px_16px_5px] border border-[var(--stroke-soft)] bg-[var(--surface-muted)] [animation:agent-message-pulse_1.9s_ease-in-out_infinite]" />
        <div className="h-[40px] w-[46%] justify-self-end rounded-[16px_16px_5px_16px] bg-[var(--user-bubble-bg)] opacity-[.14] [animation:agent-message-pulse_1.9s_ease-in-out_.25s_infinite]" />
        <div className="h-[46px] w-[58%] rounded-[16px_16px_16px_5px] border border-[var(--stroke-soft)] bg-[var(--surface-muted)] [animation:agent-message-pulse_1.9s_ease-in-out_.5s_infinite]" />
      </div>
    </div>
  )
}

function welcomeChips(t: Translate, plansAvailable: boolean) {
  return [
    {
      icon: Code2,
      label: t('chat:focusSession.explainCode'),
      prompt: t('chat:focusSession.explainHowThisCodeWorks'),
    },
    {
      icon: FlaskConical,
      label: t('chat:focusSession.writeTests'),
      prompt: t('chat:focusSession.writeUnitTestsForTheFollowingCode'),
    },
    {
      icon: Wand2,
      label: t('chat:focusSession.refactor'),
      prompt: t('chat:focusSession.refactorThisCodeAndExplainTheImprovements'),
    },
    {
      icon: Bug,
      label: t('chat:focusSession.findABug'),
      prompt: t('chat:focusSession.helpMeLocateAndFixThisBug'),
    },
    {
      icon: FileText,
      label: t('chat:focusSession.summarize'),
      prompt: t('chat:focusSession.summarizeContentAndExtractKeyPoints'),
    },
    ...(plansAvailable
      ? [
          {
            icon: ListChecks,
            label: t('chat:focusSession.makeAPlan'),
            prompt: t('chat:focusSession.makeAClearActionablePlanForThisGoal'),
          },
        ]
      : []),
    {
      icon: Layers,
      label: t('chat:focusSession.organizeInformation'),
      prompt: t('chat:focusSession.organizeThisInformationIntoAClearStructure'),
    },
    {
      icon: SearchCheck,
      label: t('chat:focusSession.findIssues'),
      prompt: t('chat:focusSession.reviewThisContentAndSuggestImprovements'),
    },
  ]
}

export function FocusTranscript({
  sessionId,
  messages,
  layoutMeasurementKey,
  transcriptLoadState = 'ready',
  messageStart,
  hasOlder,
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
  onPromptSelect,
}: FocusTranscriptProps) {
  const { t } = useI18n()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const plansAvailable = runtimeFeatureAvailable(capabilities, 'plans')
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
    hasUnread,
    scrollToBottom,
    maintainBottom,
    pauseFollowing,
  } = useAutoScroll(transcriptVersion, { resetKey: `${sessionId}:${transcriptLoadState}` })
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
    if (!node || !hasOlder || loadingOlder || prependSnapshot.current) return
    pauseFollowing()
    prependSnapshot.current = { scrollHeight: node.scrollHeight, scrollTop: node.scrollTop }
    const loaded = await onLoadOlder?.()
    if (!loaded) prependSnapshot.current = null
  }, [hasOlder, loadingOlder, onLoadOlder, pauseFollowing, transcriptRef])
  const handleTranscriptScroll = useCallback(
    (event: UIEvent<HTMLDivElement>) => {
      if (event.currentTarget.scrollTop <= 96) void loadOlder()
    },
    [loadOlder],
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

  // 轮换标题只从当前 Runtime 确认可用的能力中抽取，避免降级宿主展示无效入口。
  const welcomeTitles = useMemo(() => {
    const hour = new Date().getHours()
    const greeting =
      hour >= 23 || hour < 5
        ? t('chat:focusSession.timeLateNight')
        : hour < 11
          ? t('chat:focusSession.timeMorning')
          : hour < 14
            ? t('chat:focusSession.timeLunch')
            : hour < 18
              ? t('chat:focusSession.timeAfternoon')
              : t('chat:focusSession.timeEvening')
    const pool = [
      ...(runtimeFeatureAvailable(capabilities, 'plugins')
        ? [
            t('chat:focusSession.feelLikeBuildingAPlugin'),
            t('chat:focusSession.letAgentWriteItsOwnTool'),
          ]
        : []),
      ...(runtimeFeatureAvailable(capabilities, 'mcp')
        ? [t('chat:focusSession.wireUpAnMcpServer')]
        : []),
      ...(runtimeFeatureAvailable(capabilities, 'schedules')
        ? [t('chat:focusSession.scheduleTheRepetitiveWork')]
        : []),
      ...(runtimeFeatureAvailable(capabilities, 'workflows')
        ? [t('chat:focusSession.turnRepeatedWorkIntoAWorkflow')]
        : []),
      ...(runtimeFeatureAvailable(capabilities, 'multiAgent')
        ? [t('chat:focusSession.branchOutAndRunInParallel')]
        : []),
      ...(runtimeFeatureAvailable(capabilities, 'memory')
        ? [t('chat:focusSession.letMemoryRememberYou')]
        : []),
    ]
    // 每次挂载随机抽三条,同一会话不同时刻看到的组合也不同
    const picked = [...pool].sort(() => Math.random() - 0.5).slice(0, 3)
    return [greeting, ...picked]
  }, [capabilities, t])
  const welcomeContent = (
    <>
      <details className="group relative text-xs text-muted-foreground">
        <summary className="cursor-pointer list-none rounded-lg px-3 py-2 transition-colors hover:bg-muted hover:text-foreground">
          {t('chat:focusSession.promptIdeas')}
        </summary>
        <div className="welcome-chips flex max-w-[560px] flex-wrap justify-center gap-2 pt-2">
          {welcomeChips(t, plansAvailable).map((chip) => (
            <button
              type="button"
              key={chip.label}
              data-target-cursor
              onClick={() => onPromptSelect(chip.prompt)}
              className="inline-flex min-h-9 items-center gap-2 rounded-full border border-border bg-background px-3 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            >
              <chip.icon size={14} className="text-muted-foreground" />
              {chip.label}
            </button>
          ))}
        </div>
      </details>
    </>
  )

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
          <div className="agent-welcome relative grid min-h-full place-content-center justify-items-center text-center text-muted-foreground [[data-mobile-keyboard='open']_&]:pointer-events-none [[data-mobile-keyboard-transition='opening']_&]:pointer-events-none [[data-mobile-keyboard-transition='closing']_&]:pointer-events-none">
            <Suspense
              fallback={
                <WelcomeFallback title={welcomeTitles[0]}>{welcomeContent}</WelcomeFallback>
              }
            >
              <WelcomeEffects titles={welcomeTitles}>{welcomeContent}</WelcomeEffects>
            </Suspense>
          </div>
        )}
        {transcriptLoadState === 'ready' && messages.length > 0 && (
          <div className="[animation:transcript-reveal-enter_.22s_var(--ease-out)_both]">
            <VirtualMessageTranscript
              key={sessionId}
              layoutMeasurementKey={layoutMeasurementKey}
              sessionId={sessionId}
              messages={messages}
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
      {hasUnread && (
        <div className="pointer-events-none absolute inset-x-0 bottom-[6px] z-[5] flex justify-center">
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="pointer-events-auto bg-surface-subtle min-h-[32px] [border-color:var(--accent-border)] text-[var(--star-strong)] shadow-[0_8px_18px_-14px_var(--shadow)]"
            onClick={() => scrollToBottom('smooth')}
          >
            <ArrowDown size={14} />
            {t('chat:focusSession.newContent')}
          </Button>
        </div>
      )}
    </div>
  )
}
