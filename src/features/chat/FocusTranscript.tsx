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
  Bug,
  Code2,
  FileText,
  FlaskConical,
  FolderOpen,
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
import { workspaceName } from '@/lib/format'
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
    <div className="agent-welcome-content grid w-full max-w-[680px] place-items-center [padding:12px_20px_30px]">
      <div className="[animation:transcript-stage-enter_.5s_var(--ease-out)_both]">
        <WelcomeBrandStage />
      </div>
      <h2 className="text-[var(--accent-strong)]">{title}</h2>
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
        <strong className="text-[13px] font-[650] text-[var(--text-soft)]">{label}</strong>
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
      <p>{t('chat:focusSession.readyToWorkWithTheCurrentDirectoryAndHelpCompleteTheTask')}</p>
      <button
        type="button"
        className="welcome-workspace [&_>_svg]:flex-none [&_>_span]:flex-none [&_>_strong]:min-w-0 [&_>_strong]:overflow-hidden [&_>_strong]:text-[var(--text-soft)] [&_>_strong]:text-[12.5px] [&_>_strong]:text-ellipsis [&_>_small]:flex-none [&_>_small]:ml-[3px] [&_>_small]:text-[var(--accent-strong)] [&_>_small]:text-[12px] [&_>_small]:font-[700] hover:border-[var(--stroke-hover)] hover:bg-[var(--surface-hover)] hover:text-[var(--text)] [&:hover_>_strong]:text-[var(--text)] focus-visible:[outline:2px_solid_var(--brand-blue)] focus-visible:[outline-offset:1px] disabled:[cursor:wait] disabled:opacity-[.62] @max-[470px]:max-w-[100%] @max-[470px]:[&_>_span]:hidden flex max-w-[min(460px,100%)] min-h-[36px] items-center gap-[7px] overflow-hidden [margin-top:18px] border border-[var(--stroke)] rounded-[var(--r-pill)] bg-[var(--solid)] [padding:6px_14px] text-[var(--text-muted)] text-[12.5px] whitespace-nowrap shadow-[var(--sh-1)] [transition:background_var(--d1)_var(--ease-out),_color_var(--d1)_var(--ease-out),_border-color_var(--d1)_var(--ease-out)] [animation:transcript-stage-enter_.55s_var(--ease-out)_both] [animation-delay:240ms]"
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
      <div className="welcome-chips flex max-w-[560px] flex-wrap justify-center gap-[9px] [margin-top:24px]">
        {welcomeChips(t, plansAvailable).map((chip, index) => (
          <button
            type="button"
            key={chip.label}
            data-target-cursor
            onClick={() => onPromptSelect(chip.prompt)}
            className="group inline-flex min-h-[38px] items-center gap-[7px] rounded-[var(--r-pill)] border border-[var(--stroke)] bg-[var(--solid)] px-[16px] text-[13px] font-[620] text-[var(--text-soft)] shadow-[var(--sh-1)] [transition:all_var(--d1)_var(--ease-out)] hover:-translate-y-[1px] hover:border-[color-mix(in_srgb,#A855F7_45%,var(--stroke))] hover:text-[var(--text)] hover:shadow-[0_10px_24px_-16px_rgba(168,85,247,.5)] [animation:transcript-stage-enter_.55s_var(--ease-out)_both]"
            style={{ animationDelay: `${300 + index * 45}ms` }}
          >
            <chip.icon
              size={14}
              className="text-[var(--text-muted)] [transition:color_var(--d1)_var(--ease-out)] group-hover:text-[#A855F7]"
            />
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
          <div className="agent-welcome [&_h2]:mt-[18px] [&_h2]:text-[clamp(28px,_3vw,_38px)] [&_h2]:font-[800] [&_h2]:leading-[1.2] [&_h2]:tracking-[-.02em] [&_p]:max-w-[600px] [&_p]:mt-[14px] [&_p]:text-[15px] [&_p]:leading-[1.75] relative grid min-h-[100%] place-content-center justify-items-center overflow-hidden text-[var(--text-muted)] text-center">
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
