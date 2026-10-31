import { TASK_VIEW_WILL_CHANGE } from './events'
// 聊天页的会话视图容器：会话页只挂一个会话（SingleSessionPanel），把会话状态与回调接到 FocusSession。
// 第 3 期起分屏不再挂载，A4 删除了分屏面板、手机会话标签与分屏库依赖。
import { useContext, useEffect, useMemo, useRef, useSyncExternalStore } from 'react'
import { AlertTriangle, Plus } from 'lucide-react'
import { StarOrbit } from '@/components/StarOrbit'
import { Button } from '@/components/ui/button'
import { Kbd, KbdGroup } from '@/components/ui/kbd'
import { cn } from '@/lib/utils'
import { useI18n } from '@/app/use-i18n'
import { DEFAULT_SESSION_STATE, isPlanActive, resolveSessionPlan } from '@/lib/session-state'
import type { ChatAttachment, ResourceInvocation } from '@/types/chat'
import { TaskWorkspace } from './TaskWorkspace'
import { SessionWorkspace } from './SessionWorkspace'
import { SessionPageChromeContext } from './session-workspace-context'
import { useContextPagesStore } from './session-context-pages-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { FocusSession } from './FocusSession'
import { resolveSessionStreaming } from './session-streaming-state'
import { ChatDockContext } from './chat-dock-context'

const FOCUS_MESSAGE_PAGE_SIZE = 40
// 空数组兜底共享同一引用，避免每次渲染新建数组击穿 FocusSession 的 memo。
const EMPTY_LIST: never[] = []

// 简约工作台只挂载一个会话；切换、历史和并行后台运行仍由会话目录负责。
// 不挂载 Dockview，因此旧的分屏持久化数据不会再次打开无法关闭的窗口。
// 会话页（/board/:sessionId）传入路由里的 sessionId 作为唯一选中依据，缺省时沿用活动会话。
export function SingleSessionPanel({
  sessionId,
  onCreateSession,
}: {
  sessionId?: string
  onCreateSession: () => Promise<unknown>
}) {
  const context = useContext(ChatDockContext)
  const activeId = sessionId ?? (context?.activeId || '')
  if (!activeId) return <ChatDockWatermark onNewSession={onCreateSession} />
  return (
    <SessionPanel
      key={activeId}
      sessionId={activeId}
      panelId={`single-${activeId}`}
      shortcutEnabled
    />
  )
}

type SessionPanelProps = {
  sessionId: string
  panelId: string
  shortcutEnabled: boolean
  onFocusCapture?: React.FocusEventHandler<HTMLDivElement>
}

// 会话面板主体：Dock 面板与移动端单会话视图共用，
// 只负责把会话状态与回调接到 FocusSession，不关心外层布局容器。
function SessionPanel({ sessionId, panelId, shortcutEnabled, onFocusCapture }: SessionPanelProps) {
  const context = useContext(ChatDockContext)
  const pageChrome = useContext(SessionPageChromeContext)
  const capabilities = useRuntimeCapabilitiesStore((store) => store.capabilities)
  const contentOpen = useContextPagesStore(
    (store) => store.sessions[sessionId]?.contentOpen || false,
  )
  const updateWorkspace = useContextPagesStore((store) => store.update)

  // 面板只订阅自己会话的 state：其它会话的流式帧不会触发本面板重渲染。
  const subscribeSessionState = context?.subscribeSessionState
  const getSessionState = context?.getSessionState
  const subscribe = useMemo(
    () => (listener: () => void) => subscribeSessionState?.(sessionId, listener) ?? (() => {}),
    [subscribeSessionState, sessionId],
  )
  const getSnapshot = useMemo(
    () => () => getSessionState?.(sessionId),
    [getSessionState, sessionId],
  )
  const sessionState = useSyncExternalStore(subscribe, getSnapshot)
  const session = context?.sessions.find((item) => item.id === sessionId)
  const state = sessionState || DEFAULT_SESSION_STATE
  const streaming = resolveSessionStreaming(state, session)
  const plan = resolveSessionPlan(sessionState, session)
  const team =
    sessionState && Object.hasOwn(sessionState, 'team')
      ? sessionState.team
      : (session?.team ?? null)
  const visiblePlan = isPlanActive(plan, { streaming }) ? plan : null
  const sessionTreePulse =
    context?.sessionTreePulseSessionId === sessionId ? context.sessionTreePulseToken : 0
  const loadMessages = context?.loadSessionMessages
  const loadThinkingLevel = context?.loadSessionThinkingLevel
  const thinkingRequestedRef = useRef('')

  useEffect(() => {
    if (sessionId) void loadMessages?.(sessionId, { limit: FOCUS_MESSAGE_PAGE_SIZE })
  }, [loadMessages, sessionId])

  // Self-heal thinking state for sessions loaded while streaming or after page remounts.
  useEffect(() => {
    if (!sessionId || !session || !loadThinkingLevel) return
    if (state.thinkingStatus) return
    if ((state.availableThinkingLevels || []).length) return
    if (thinkingRequestedRef.current === sessionId) return
    thinkingRequestedRef.current = sessionId
    void loadThinkingLevel(sessionId)
  }, [
    loadThinkingLevel,
    session,
    sessionId,
    state.availableThinkingLevels,
    state.thinkingStatus,
    streaming,
  ])

  // 交给 FocusSession 的回调统一 memo：context value 已稳定，
  // 仅会话/面板标识变化时重建，配合 FocusSession 的 memo 跳过无关重渲染。
  // 回调只会在 context 与 session 都存在（即 FocusSession 已渲染）时触发，
  // 这里的可选链兜底仅为满足类型。
  const handlers = useMemo(
    () => ({
      onLoadLatest: () => context?.loadSessionMessages(sessionId, { force: true }),
      onLoadOlder: () => context?.loadOlderMessages(sessionId) ?? false,
      onModelChange: (nextModel: string) => context?.switchSessionModel(sessionId, nextModel),
      onThinkingLevelChange: (nextLevel: string) =>
        context?.switchSessionThinkingLevel(sessionId, nextLevel),
      onExecutionModeChange: (nextMode: string) =>
        context?.switchSessionExecutionMode(sessionId, nextMode) ?? false,
      onRunModeChange: (nextMode: string) =>
        context?.switchSessionRunMode(sessionId, nextMode) ?? false,
      onGoalPause: () => context?.pauseGoal(sessionId),
      onGoalBudgetChange: (tokenBudget: number | null) =>
        context?.setGoalBudget(sessionId, tokenBudget),
      onCompact: () => context?.compactSession(sessionId),
      onApproval: (approvalId: string, approved: boolean) =>
        context?.resolveToolApproval(sessionId, approvalId, approved),
      onAnswerQuestion: async (
        questionId: string,
        body: { answers: { selected: string[]; other?: string }[] } | { skip: true },
      ) => {
        await context?.answerQuestion(sessionId, questionId, body)
      },
      onWorkspace: () => {
        if (session) void context?.selectSessionWorkspace(session)
      },
      onRename: () => {
        if (session) void context?.renameSession(session)
      },
      onBranchFromHere: (boundaryEntryId: string) => {
        if (session) return context?.branchFromEntry(session, boundaryEntryId)
      },
      onCreateChildSession: (boundaryEntryId: string) => {
        if (session) return context?.createChildSession(session, boundaryEntryId)
      },
      onRetryLastTurn: () => context?.retryLastTurn(sessionId),
      onTreeNavigated: () => context?.reloadSessionBranch(sessionId),
      onToggleContext: (open: boolean) => {
        window.dispatchEvent(new Event(TASK_VIEW_WILL_CHANGE))
        const workspace = useContextPagesStore.getState().sessions[sessionId]
        if (open && !workspace?.pages.length)
          updateWorkspace(sessionId, 'files', { type: 'open', kind: 'files' })
        else updateWorkspace(sessionId, 'files', { type: 'workspace', contentOpen: open })
      },
      onSend: (
        value: string,
        attachments: ChatAttachment[],
        goalMode: boolean,
        teamMode: boolean,
        goalTokenBudget: number | null,
        invocation?: ResourceInvocation | null,
        onAccepted?: () => void,
      ) =>
        context?.sendPrompt(
          value,
          sessionId,
          attachments,
          goalMode,
          teamMode,
          goalTokenBudget,
          invocation,
          { onAccepted },
        ),
      onQueue: (
        value: string,
        attachments: ChatAttachment[],
        behavior: string,
        onAccepted?: () => void,
      ) => context?.queuePrompt(value, sessionId, attachments, behavior, onAccepted) ?? false,
      onWithdrawQueuedInput: (inputId: string) =>
        context?.withdrawQueuedInput(sessionId, inputId) ?? Promise.resolve(null),
      onAbort: () => context?.abort(sessionId),
    }),
    [context, sessionId, session, panelId, updateWorkspace],
  )

  if (!context || !session) {
    return (
      <div className="session-dock-missing flex h-full min-h-[180px] items-center justify-center gap-2 text-sm text-content-secondary">
        <AlertTriangle size={16} />
        {context ? 'Session unavailable' : 'Loading session'}
      </div>
    )
  }
  const pending =
    context.pendingAsset?.asset &&
    (!context.pendingAsset.targetSessionId || context.pendingAsset.targetSessionId === sessionId)
      ? context.pendingAsset.asset
      : null
  const messages = state.historyWindow?.messages || state.messages || EMPTY_LIST
  const loadState = state.loaded ? 'ready' : state.loading || !state.error ? 'loading' : 'error'
  // 运行中的会话在 /live 给出消息之前保持“加载中”：空转录在这里只可能是还没读到，
  // 不能显示空会话的问候页（别处发起的运行打开时会先闪一下问候页再跳成对话）。
  const awaitingLive = Boolean(state.recovering && !messages.length && !state.error)
  const completed = Boolean(
    state.lifecycle?.phase === 'completed' && !state.error && !state.runStopped,
  )
  const runStartedAt = typeof state.runStartedAt === 'string' ? state.runStartedAt : null
  const goal = state.goal ?? session.goal ?? null
  const focus = (
    <FocusSession
      session={session}
      shortcutEnabled={shortcutEnabled}
      messages={messages}
      transcriptLoadState={awaitingLive ? 'loading' : loadState}
      hasNewer={Boolean(state.historyWindow)}
      messageStart={state.historyWindow?.messageStart ?? state.messageStart}
      hasOlder={state.historyWindow?.hasOlder ?? state.hasOlder}
      loadingOlder={state.loadingOlder}
      olderError={state.olderError}
      model={state.model || session.model || context.defaultModel}
      thinkingLevel={state.thinkingLevel || session.thinkingLevel || 'medium'}
      availableThinkingLevels={state.availableThinkingLevels || EMPTY_LIST}
      thinkingStatus={state.thinkingStatus || ''}
      thinkingMessage={state.thinkingMessage || ''}
      executionMode={state.executionMode || session.executionMode || 'approval-required'}
      runMode={state.runMode || session.runMode || 'plan'}
      goal={goal}
      team={team}
      plan={visiblePlan}
      currentActivity={state.currentActivity}
      activityFeed={state.activityFeed || EMPTY_LIST}
      tools={state.tools || EMPTY_LIST}
      thinkingText={state.thinkingText || ''}
      queuedInputs={state.queuedInputs || EMPTY_LIST}
      withdrawingInputIds={state.withdrawingInputIds || EMPTY_LIST}
      compaction={state.compaction}
      contextUsage={state.contextUsage}
      sessionUsage={state.sessionUsage}
      sessionTreeRevision={state.sessionTreeRevision}
      sessionTreePulse={sessionTreePulse}
      cwd={state.cwd || session.cwd}
      availableModels={context.availableModels}
      pendingRuntimeSelection={state.pendingRuntimeSelection}
      switchingModel={state.switchingModel}
      switchingThinking={state.switchingThinking}
      switchingCwd={state.switchingCwd}
      switchingPermission={state.switchingPermission}
      streaming={streaming}
      runStartedAt={state.runStartedAt}
      lastActivityAt={state.lastActivityAt}
      runFinishedAt={state.runFinishedAt}
      runStopped={state.runStopped}
      runNotice={state.runNotice}
      approvals={state.approvals || EMPTY_LIST}
      questions={state.questions || EMPTY_LIST}
      error={state.error || (context.activeId === sessionId ? context.globalError : '')}
      pendingAsset={pending}
      onAssetConsumed={context.onAssetConsumed}
      notify={context.notify}
      onOpenModelSettings={context.openModelSettings}
      onCompactionThresholdChange={context.setCompactionThreshold}
      contextOpen={contentOpen}
      contextCompact={context.mobileLayout}
      contextPanelId={`${panelId || `mobile-${sessionId}`}-context`}
      {...handlers}
    />
  )
  return (
    <div
      className={cn(
        'session-dock-panel w-full h-full min-w-0 min-h-0 overflow-hidden [container-type:inline-size] bg-panel',
        // 切换代理时对话区淡入 120ms（只动 opacity，减少动效时由全局规则关闭）。
        pageChrome && 'animate-[fade-in_var(--d-fast)_var(--ease-enter)]',
      )}
      onFocusCapture={onFocusCapture}
      data-session-panel={sessionId}
    >
      {pageChrome ? (
        // 会话页（桌面布局）：“页头 + 对话 + 检查器”的会话工作区；其余入口保持原来的任务工作区。
        <SessionWorkspace
          key={sessionId}
          sessionId={sessionId}
          messages={messages}
          cwd={state.cwd || session.cwd || ''}
          active={shortcutEnabled}
          plan={visiblePlan}
          goal={goal}
          team={team}
          activity={state.activityFeed?.length ? state.activityFeed : state.tools || EMPTY_LIST}
          planTab={Boolean(visiblePlan?.items?.length || goal || team)}
          streaming={streaming}
          completed={completed}
          runStartedAt={runStartedAt}
          requestConfirm={context.requestConfirm}
        >
          {focus}
        </SessionWorkspace>
      ) : (
        <TaskWorkspace
          messages={messages}
          taskName={session.name || sessionId}
          cwd={state.cwd || session.cwd || ''}
          key={sessionId}
          sessionId={sessionId}
          panelId={panelId || `mobile-${sessionId}`}
          active={shortcutEnabled}
          mobile={context.mobileLayout}
          plan={visiblePlan}
          streaming={streaming}
          completed={completed}
          runStartedAt={runStartedAt}
          plansAvailable={runtimeFeatureAvailable(capabilities, 'plans')}
          requestConfirm={context.requestConfirm}
        >
          {focus}
        </TaskWorkspace>
      )}
    </div>
  )
}

type ChatDockWatermarkProps = {
  // 空 Dock 的主行动作：提供时渲染醒目的新建会话按钮，否则只展示纯提示。
  onNewSession?: () => void | Promise<unknown>
  // 新建会话的快捷键提示（桌面端 Ctrl/⌘ N），移动端没有快捷键、不传。
  newSessionShortcut?: string
}

export function ChatDockWatermark({ onNewSession, newSessionShortcut }: ChatDockWatermarkProps) {
  const { t } = useI18n()
  // 空 Dock 是品牌时刻：星轨插画 + 14/500 标题 + 12px 说明；新建会话是键帽按钮，快捷键画成小键帽。
  return (
    <div className="flex h-full min-h-[180px] flex-col items-center justify-center gap-1.5 px-6 text-center">
      <StarOrbit className="mb-1.5" />
      <strong className="text-sm font-medium text-content">
        {t('chat:chatDock.openAChatToBegin')}
      </strong>
      <span className="max-w-[320px] text-xs leading-relaxed text-content-muted">
        {t('chat:chatDock.openAChatFromTheSessionListOrSplitItInAnyDirection')}
      </span>
      {onNewSession ? (
        <Button variant="secondary" className="mt-3" onClick={() => void onNewSession()}>
          <Plus aria-hidden="true" />
          {t('chat:chatDock.newChat')}
          {newSessionShortcut ? (
            <KbdGroup aria-hidden="true" className="ml-1.5 pointer-coarse:hidden">
              {newSessionShortcut.split(/\+(?=.)/).map((key) => (
                <Kbd key={key}>{key}</Kbd>
              ))}
            </KbdGroup>
          ) : null}
        </Button>
      ) : null}
    </div>
  )
}
