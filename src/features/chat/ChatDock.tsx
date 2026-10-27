import { TASK_VIEW_WILL_CHANGE } from './events'
// 聊天 Dock 面板容器：每个 dockview 面板对应一个会话视图，
// 负责把面板事件（关闭/激活）桥接到会话状态与布局持久化。
import { useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import type { IDockviewPanelProps } from 'dockview-react'
import { AlertTriangle, MessageSquare, Plus } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { DEFAULT_SESSION_STATE, isPlanActive, resolveSessionPlan } from '@/lib/session-state'
import type { ChatAttachment, ResourceInvocation } from '@/types/chat'
import { TaskWorkspace } from './TaskWorkspace'
import { useContextPagesStore } from './session-context-pages-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { FocusSession } from './FocusSession'
import { resolveSessionStreaming } from './session-streaming-state'
import { ChatDockContext } from './chat-dock-context'
import { useMobileShell } from '@/components/layout/MobileShellContext'
import { sessionIdFromPanel } from './dock-layout'

const FOCUS_MESSAGE_PAGE_SIZE = 40
// 空数组兜底共享同一引用，避免每次渲染新建数组击穿 FocusSession 的 memo。
const EMPTY_LIST: never[] = []

export function SessionDockPanel({ params, api }: IDockviewPanelProps<{ sessionId?: string }>) {
  const context = useContext(ChatDockContext)
  const sessionId = params?.sessionId || sessionIdFromPanel(api?.id)
  const [visible, setVisible] = useState(() => api.isVisible)
  const [active, setActive] = useState(() => api.isActive)

  useEffect(() => {
    setActive(api.isActive)
    const disposable = api.onDidActiveChange(({ isActive }) => setActive(isActive))
    return () => disposable.dispose()
  }, [api])

  useEffect(() => {
    setVisible(api.isVisible)
    const disposable = api.onDidVisibilityChange(({ isVisible }) => setVisible(isVisible))
    return () => disposable.dispose()
  }, [api])

  if (!visible) return null
  return (
    <SessionPanel
      sessionId={sessionId}
      panelId={api.id}
      shortcutEnabled={active}
      onFocusCapture={(event) => {
        const root = event.currentTarget
        // Portal 菜单的焦点也会沿 React 树冒泡；不能由旧菜单重新激活已切走的任务。
        if (!root.contains(event.target)) return
        if (!api.isActive)
          queueMicrotask(() => {
            if (api.isVisible && root.contains(document.activeElement)) api.setActive()
          })
      }}
      canSplitPanel={Boolean(context?.compactDock === false && api.group.size > 1)}
      canClosePanel
    />
  )
}

// 简约工作台只挂载活动会话；切换、历史和并行后台运行仍由会话目录负责。
// 不挂载 Dockview，因此旧的分屏持久化数据不会再次打开无法关闭的窗口。
export function SingleSessionPanel({
  onCreateSession,
}: {
  onCreateSession: () => Promise<unknown>
}) {
  const context = useContext(ChatDockContext)
  const activeId =
    context?.sessions.find((session) => session.id === context.activeId)?.id ||
    context?.sessions[0]?.id ||
    ''
  return (
    <div
      className="single-session-shell flex h-full min-h-0 min-w-0 flex-col"
      data-single-session={activeId}
    >
      {activeId ? (
        <SessionPanel
          key={activeId}
          sessionId={activeId}
          panelId={`single-${activeId}`}
          shortcutEnabled
          canSplitPanel={false}
          canClosePanel={false}
        />
      ) : (
        <ChatDockWatermark onNewSession={onCreateSession} mobile />
      )}
    </div>
  )
}

type SessionPanelProps = {
  sessionId: string
  panelId: string
  shortcutEnabled: boolean
  onFocusCapture?: React.FocusEventHandler<HTMLDivElement>
  canSplitPanel: boolean
  canClosePanel: boolean
}

// 会话面板主体：Dock 面板与移动端单会话视图共用，
// 只负责把会话状态与回调接到 FocusSession，不关心外层布局容器。
function SessionPanel({
  sessionId,
  panelId,
  shortcutEnabled,
  onFocusCapture,
  canSplitPanel,
  canClosePanel,
}: SessionPanelProps) {
  const context = useContext(ChatDockContext)
  const capabilities = useRuntimeCapabilitiesStore((store) => store.capabilities)
  const shell = useMobileShell()
  const storedContentOpen = useContextPagesStore(
    (store) => store.sessions[sessionId]?.contentOpen || false,
  )
  const contentOpen = shell.active
    ? shortcutEnabled && shell.tab === 'session-tools' && shell.pane === 'context'
    : storedContentOpen
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
  useEffect(() => {
    const report = context?.reportActiveSessionRun
    if (!shortcutEnabled || !report) return
    report({ sessionId, streaming })
    return () => report(null)
  }, [context?.reportActiveSessionRun, sessionId, shortcutEnabled, streaming])
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
      onSplitLeft: () => context?.splitDockPanel(panelId, 'left'),
      onSplitRight: () => context?.splitDockPanel(panelId, 'right'),
      onSplitTop: () => context?.splitDockPanel(panelId, 'above'),
      onSplitBottom: () => context?.splitDockPanel(panelId, 'below'),
      onClosePanel: () => {
        window.dispatchEvent(new Event(TASK_VIEW_WILL_CHANGE))
        context?.closeDockPanel(panelId)
      },
      onToggleContext: (open: boolean) => {
        window.dispatchEvent(new Event(TASK_VIEW_WILL_CHANGE))
        const workspace = useContextPagesStore.getState().sessions[sessionId]
        if (open && !workspace?.pages.length)
          updateWorkspace(sessionId, 'files', { type: 'open', kind: 'files' })
        else updateWorkspace(sessionId, 'files', { type: 'workspace', contentOpen: open })
        if (!shell.active) return
        if (open) shell.openContext('session-tools')
        else shell.setPane('chat')
      },
      onSend: (
        value: string,
        attachments: ChatAttachment[],
        goalMode: boolean,
        teamMode: boolean,
        goalTokenBudget: number | null,
        invocation?: ResourceInvocation | null,
      ) =>
        context?.sendPrompt(
          value,
          sessionId,
          attachments,
          goalMode,
          teamMode,
          goalTokenBudget,
          invocation,
        ),
      onQueue: (value: string, attachments: ChatAttachment[], behavior: string) =>
        context?.queuePrompt(value, sessionId, attachments, behavior) ?? false,
      onWithdrawQueuedInput: (inputId: string) =>
        context?.withdrawQueuedInput(sessionId, inputId) ?? Promise.resolve(null),
      onAbort: () => context?.abort(sessionId),
    }),
    [context, sessionId, session, panelId, updateWorkspace, shell],
  )

  if (!context || !session) {
    return (
      <div className="session-dock-missing flex h-full min-h-[180px] items-center justify-center gap-[8px] text-[var(--text-secondary)] text-[length:var(--app-font-size)]">
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
  return (
    <div
      className="session-dock-panel w-full h-full min-w-0 min-h-0 overflow-hidden [container-type:inline-size] bg-[var(--panel)]"
      onFocusCapture={onFocusCapture}
    >
      <TaskWorkspace
        messages={state.messages || EMPTY_LIST}
        taskName={session.name || sessionId}
        cwd={state.cwd || session.cwd || ''}
        key={sessionId}
        sessionId={sessionId}
        panelId={panelId || `mobile-${sessionId}`}
        active={shortcutEnabled}
        mobile={context.mobileLayout}
        plan={visiblePlan}
        streaming={streaming}
        completed={Boolean(
          state.lifecycle?.phase === 'completed' && !state.error && !state.runStopped,
        )}
        runStartedAt={typeof state.runStartedAt === 'string' ? state.runStartedAt : null}
        plansAvailable={runtimeFeatureAvailable(capabilities, 'plans')}
        requestConfirm={context.requestConfirm}
      >
        <FocusSession
          session={session}
          shortcutEnabled={shortcutEnabled}
          messages={state.messages || EMPTY_LIST}
          transcriptLoadState={
            state.loaded ? 'ready' : state.loading || !state.error ? 'loading' : 'error'
          }
          messageStart={state.messageStart}
          hasOlder={state.hasOlder}
          loadingOlder={state.loadingOlder}
          olderError={state.olderError}
          model={state.model || session.model || context.defaultModel}
          thinkingLevel={state.thinkingLevel || session.thinkingLevel || 'medium'}
          availableThinkingLevels={state.availableThinkingLevels || EMPTY_LIST}
          thinkingStatus={state.thinkingStatus || ''}
          thinkingMessage={state.thinkingMessage || ''}
          executionMode={state.executionMode || session.executionMode || 'approval-required'}
          runMode={state.runMode || session.runMode || 'plan'}
          goal={state.goal ?? session.goal ?? null}
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
          error={state.error || (context.activeId === sessionId ? context.globalError : '')}
          pendingAsset={pending}
          onAssetConsumed={context.onAssetConsumed}
          notify={context.notify}
          onOpenModelSettings={context.openModelSettings}
          onCompactionThresholdChange={context.setCompactionThreshold}
          canSplit={canSplitPanel}
          canClosePanel={canClosePanel}
          contextOpen={contentOpen}
          contextCompact={context.mobileLayout}
          contextPanelId={`${panelId || `mobile-${sessionId}`}-context`}
          {...handlers}
        />
      </TaskWorkspace>
    </div>
  )
}

type ChatDockWatermarkProps = {
  // 空 Dock 的主行动作：提供时渲染醒目的新建会话按钮，否则只展示纯提示。
  onNewSession?: () => void | Promise<unknown>
  // 新建会话的快捷键提示（桌面端 Ctrl/⌘ N），移动端没有快捷键、不传。
  newSessionShortcut?: string
  mobile?: boolean
}

export function ChatDockWatermark({
  onNewSession,
  newSessionShortcut,
  mobile = false,
}: ChatDockWatermarkProps) {
  const { t } = useI18n()
  return (
    <div className="flex h-full min-h-[180px] flex-col items-center justify-center gap-[8px] text-center text-[length:var(--app-font-size)] text-[var(--text-secondary)]">
      <MessageSquare size={34} />
      <strong className="text-[14px] font-semibold text-[var(--text)]">
        {t('chat:chatDock.openAChatToBegin')}
      </strong>
      <span className="max-w-[320px] leading-[1.55]">
        {mobile
          ? t('chat:chatDock.openAChatFromTheSessionList')
          : t('chat:chatDock.openAChatFromTheSessionListOrSplitItInAnyDirection')}
      </span>
      {onNewSession ? (
        <button
          type="button"
          className="mt-[10px] inline-flex cursor-pointer items-center gap-[6px] rounded-[var(--r-sm)] border-0 bg-[var(--brand-blue)] px-[16px] py-[8px] text-[length:var(--app-font-size)] font-medium text-white transition-colors duration-[var(--d1)] hover:bg-[var(--brand-blue-hover)] focus-visible:outline-[2px] focus-visible:outline-offset-2 focus-visible:outline-[var(--focus)]"
          onClick={() => void onNewSession()}
        >
          <Plus size={14} aria-hidden="true" />
          {t('chat:chatDock.newChat')}
          {newSessionShortcut ? (
            <kbd className="ml-[2px] rounded-[4px] border border-white/35 bg-white/10 px-[5px] py-[1px] font-sans text-[length:var(--app-small-size)] font-medium leading-[1.4]">
              {newSessionShortcut}
            </kbd>
          ) : null}
        </button>
      ) : null}
    </div>
  )
}
