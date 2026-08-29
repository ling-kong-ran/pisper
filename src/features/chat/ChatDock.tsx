// 聊天 Dock 面板容器：每个 dockview 面板对应一个会话视图，
// 负责把面板事件（关闭/激活）桥接到会话状态与布局持久化。
import { useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import type { IDockviewPanelProps } from 'dockview-react'
import { AlertTriangle, MessageSquare } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { DEFAULT_SESSION_STATE, isPlanActive, resolveSessionPlan } from '@/lib/session-state'
import type { ChatAttachment, ResourceInvocation } from '@/types/chat'
import { FocusSession } from './FocusSession'
import { ChatDockContext } from './chat-dock-context'
import { sessionIdFromPanel } from './dock-layout'

const FOCUS_MESSAGE_PAGE_SIZE = 40
// 空数组兜底共享同一引用，避免每次渲染新建数组击穿 FocusSession 的 memo。
const EMPTY_LIST: never[] = []

export function SessionDockPanel({ params, api }: IDockviewPanelProps<{ sessionId?: string }>) {
  const context = useContext(ChatDockContext)
  const sessionId = params?.sessionId || sessionIdFromPanel(api?.id)
  const [visible, setVisible] = useState(() => api.isVisible)

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
      onFocusCapture={() => api.setActive()}
      canSplitPanel={Boolean(context?.compactDock === false && api.group.size > 1)}
      canClosePanel
    />
  )
}

// 移动端单会话面板：无 Dock 分屏，直接渲染活动会话；无会话时显示水位线。
export function MobileSessionPanel() {
  const context = useContext(ChatDockContext)
  const sessionId = context?.activeId || ''
  if (!sessionId) return <ChatDockWatermark />
  return (
    <SessionPanel sessionId={sessionId} panelId="" canSplitPanel={false} canClosePanel={false} />
  )
}

type SessionPanelProps = {
  sessionId: string
  panelId: string
  onFocusCapture?: () => void
  canSplitPanel: boolean
  canClosePanel: boolean
}

// 会话面板主体：Dock 面板与移动端单会话视图共用，
// 只负责把会话状态与回调接到 FocusSession，不关心外层布局容器。
function SessionPanel({
  sessionId,
  panelId,
  onFocusCapture,
  canSplitPanel,
  canClosePanel,
}: SessionPanelProps) {
  const context = useContext(ChatDockContext)
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
  const streaming = Boolean(state.streaming || session?.streaming)
  const plan = resolveSessionPlan(sessionState, session)
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
    if (streaming || state.thinkingStatus) return
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
      onGoalPause: () => context?.pauseGoal(sessionId),
      onGoalBudgetChange: (tokenBudget: number) => context?.setGoalBudget(sessionId, tokenBudget),
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
      onTreeNavigated: () => context?.reloadSessionBranch(sessionId),
      onSplitLeft: () => context?.splitDockPanel(panelId, 'left'),
      onSplitRight: () => context?.splitDockPanel(panelId, 'right'),
      onSplitTop: () => context?.splitDockPanel(panelId, 'above'),
      onSplitBottom: () => context?.splitDockPanel(panelId, 'below'),
      onClosePanel: () => context?.closeDockPanel(panelId),
      onSend: (
        value: string,
        attachments: ChatAttachment[],
        goalMode: boolean,
        goalTokenBudget: number | null,
        invocation?: ResourceInvocation | null,
      ) =>
        context?.sendPrompt(value, sessionId, attachments, goalMode, goalTokenBudget, invocation),
      onQueue: (value: string, attachments: ChatAttachment[], behavior: string) =>
        context?.queuePrompt(value, sessionId, attachments, behavior) ?? false,
      onAbort: () => context?.abort(sessionId),
    }),
    [context, sessionId, session, panelId],
  )

  if (!context || !session) {
    return (
      <div className="session-dock-missing flex h-full min-h-[180px] items-center justify-center gap-[8px] text-[var(--text-muted)] text-[12px]">
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
      <FocusSession
        session={session}
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
        goal={state.goal ?? session.goal ?? null}
        plan={visiblePlan}
        currentActivity={state.currentActivity}
        activityFeed={state.activityFeed || EMPTY_LIST}
        tools={state.tools || EMPTY_LIST}
        thinkingText={state.thinkingText || ''}
        queuedInputs={state.queuedInputs || EMPTY_LIST}
        compaction={state.compaction}
        contextUsage={state.contextUsage}
        sessionUsage={state.sessionUsage}
        sessionTreeRevision={state.sessionTreeRevision}
        sessionTreePulse={sessionTreePulse}
        cwd={state.cwd || session.cwd}
        availableModels={context.availableModels}
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
        {...handlers}
      />
    </div>
  )
}

export function ChatDockWatermark() {
  const { t } = useI18n()
  return (
    <div className="chat-dock-watermark flex h-full min-h-[180px] items-center justify-center gap-[8px] text-[var(--text-muted)] text-[12px] [&_strong]:text-[var(--text)] [&_strong]:text-[14px] [&_span]:max-w-[320px] [&_span]:leading-[1.55] flex-col text-center">
      <MessageSquare size={34} />
      <strong>{t('chat:chatDock.openAChatToBegin')}</strong>
      <span>{t('chat:chatDock.openAChatFromTheSessionListOrSplitItInAnyDirection')}</span>
    </div>
  )
}
