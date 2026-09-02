// 聊天 Dock 面板容器：每个 dockview 面板对应一个会话视图，
// 负责把面板事件（关闭/激活）桥接到会话状态与布局持久化。
import { useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import type { IDockviewPanelProps } from 'dockview-react'
import { AlertTriangle, MessageSquare, Plus, X } from 'lucide-react'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import { DEFAULT_SESSION_STATE, isPlanActive, resolveSessionPlan } from '@/lib/session-state'
import type { ChatAttachment, ResourceInvocation } from '@/types/chat'
import { FocusSession } from './FocusSession'
import { ChatDockContext } from './chat-dock-context'
import { closeMobileSessionTab, sessionIdFromPanel } from './dock-layout'

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

type MobileSessionPanelProps = {
  sessionIds?: string[]
  onSelectSession: (sessionId: string) => void
  onCreateSession: () => void | Promise<unknown>
}

const MOBILE_SESSION_TAB_LIMIT = 6

function readMobileSessionTabs() {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(STORAGE_KEYS.mobileSessionTabs) || '[]')
    return Array.isArray(value)
      ? value
          .filter((item): item is string => typeof item === 'string')
          .slice(-MOBILE_SESSION_TAB_LIMIT)
      : []
  } catch {
    return []
  }
}

// 移动端保留可横向浏览的会话标签，但内容区一次只挂载一个会话，
// 避免为了标签交互把桌面 Dockview 与分屏布局带入 App。
export function MobileSessionPanel({
  sessionIds = [],
  onSelectSession,
  onCreateSession,
}: MobileSessionPanelProps) {
  const { t } = useI18n()
  const context = useContext(ChatDockContext)
  const activeId = context?.activeId || ''
  const [tabIds, setTabIds] = useState(readMobileSessionTabs)
  const seededSessionIdsRef = useRef(false)
  const activeTabRef = useRef<HTMLButtonElement>(null)
  const tabs = useMemo(() => {
    const sessionsById = new Map(context?.sessions.map((session) => [session.id, session]))
    return tabIds.flatMap((id) => {
      const session = sessionsById.get(id)
      return session ? [session] : []
    })
  }, [context?.sessions, tabIds])
  const activeSession = context?.sessions.find((session) => session.id === activeId)
  const visibleTabs =
    activeSession && !tabs.some((session) => session.id === activeSession.id)
      ? [...tabs, activeSession]
      : tabs
  // 页面恢复时 localStorage 里的活动 id 可能暂时不在新目录中，不能把它直接交给面板。
  const sessionId = activeSession?.id || visibleTabs[0]?.id || ''

  useEffect(() => {
    const knownIds = new Set(context?.sessions.map((session) => session.id))
    const seedIds = !seededSessionIdsRef.current && sessionIds.length ? sessionIds : []
    if (seedIds.length) seededSessionIdsRef.current = true
    setTabIds((current) => {
      const next = current.filter((id) => knownIds.has(id))
      for (const id of [...seedIds, activeId]) {
        if (id && knownIds.has(id) && !next.includes(id)) next.push(id)
      }
      const limited = next.slice(-MOBILE_SESSION_TAB_LIMIT)
      return limited.length === current.length &&
        limited.every((id, index) => id === current[index])
        ? current
        : limited
    })
  }, [activeId, context?.sessions, sessionIds])

  useEffect(() => {
    localStorage.setItem(STORAGE_KEYS.mobileSessionTabs, JSON.stringify(tabIds))
  }, [tabIds])

  useEffect(() => {
    activeTabRef.current?.scrollIntoView({ block: 'nearest', inline: 'center' })
  }, [sessionId])

  const closeSessionTab = (closingId: string) => {
    const result = closeMobileSessionTab(
      visibleTabs.map((session) => session.id),
      closingId,
      sessionId,
    )
    setTabIds(result.tabIds)
    if (result.activeId !== sessionId) onSelectSession(result.activeId)
  }

  return (
    <div className="mobile-session-shell flex h-full min-h-0 min-w-0 flex-col bg-[var(--panel)]">
      <nav
        className="mobile-session-tabs flex h-[52px] flex-none border-b border-[var(--stroke-soft)] bg-[var(--surface-subtle)]"
        role="tablist"
        aria-label={t('chat:chatPage.openChats')}
      >
        <div className="flex min-w-0 flex-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {visibleTabs.map((session) => {
            const title = session.name || t('chat:chatPage.untitledChat')
            const active = session.id === sessionId
            return (
              <div
                className={`group relative flex h-full min-w-[148px] max-w-[228px] flex-none items-stretch border-b-[3px] transition-colors ${active ? 'border-[var(--brand-blue)] bg-[var(--solid)] text-[var(--text)]' : 'border-transparent bg-transparent text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text)]'}`}
                key={session.id}
              >
                <button
                  ref={active ? activeTabRef : undefined}
                  type="button"
                  role="tab"
                  aria-selected={active}
                  className={`flex min-w-0 flex-1 items-center gap-2 border-0 bg-transparent py-0 pl-3 text-left text-[13px] focus-visible:z-[1] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--focus)] ${active ? 'font-[650]' : 'font-[500]'}`}
                  title={title}
                  onClick={() => onSelectSession(session.id)}
                >
                  <MessageSquare size={15} className="flex-none" aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate">{title}</span>
                  {session.streaming && (
                    <span
                      aria-hidden="true"
                      className="h-1.5 w-1.5 flex-none rounded-full bg-[var(--brand-blue)]"
                    />
                  )}
                </button>
                <button
                  type="button"
                  className="grid w-9 flex-none place-items-center border-0 bg-transparent text-[var(--text-muted)] hover:bg-[var(--surface-hover)] hover:text-[var(--text)] focus-visible:z-[1] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--focus)]"
                  title={t('chat:chatPage.closeChat', { title })}
                  aria-label={t('chat:chatPage.closeChat', { title })}
                  onClick={() => closeSessionTab(session.id)}
                >
                  <X size={14} aria-hidden="true" />
                </button>
              </div>
            )
          })}
        </div>
        <button
          type="button"
          className="grid h-full w-[52px] flex-none place-items-center border-0 border-l border-[var(--stroke-soft)] bg-transparent text-[var(--text-muted)] hover:bg-[var(--surface-hover)] hover:text-[var(--text)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--focus)]"
          title={t('chat:chatPage.newChat')}
          aria-label={t('chat:chatPage.newChat')}
          onClick={() => void onCreateSession()}
        >
          <Plus size={20} aria-hidden="true" />
        </button>
      </nav>
      <div className="min-h-0 min-w-0 flex-1">
        {sessionId ? (
          <SessionPanel
            sessionId={sessionId}
            panelId=""
            canSplitPanel={false}
            canClosePanel={false}
          />
        ) : (
          <ChatDockWatermark onNewSession={onCreateSession} />
        )}
      </div>
    </div>
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
        team={team}
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

type ChatDockWatermarkProps = {
  // 空 Dock 的主行动作：提供时渲染醒目的新建会话按钮，否则只展示纯提示。
  onNewSession?: () => void | Promise<unknown>
  // 新建会话的快捷键提示（桌面端 Ctrl/⌘ N），移动端没有快捷键、不传。
  newSessionShortcut?: string
}

export function ChatDockWatermark({ onNewSession, newSessionShortcut }: ChatDockWatermarkProps) {
  const { t } = useI18n()
  return (
    <div className="flex h-full min-h-[180px] flex-col items-center justify-center gap-[8px] text-center text-[12px] text-[var(--text-muted)]">
      <MessageSquare size={34} />
      <strong className="text-[14px] text-[var(--text)]">
        {t('chat:chatDock.openAChatToBegin')}
      </strong>
      <span className="max-w-[320px] leading-[1.55]">
        {t('chat:chatDock.openAChatFromTheSessionListOrSplitItInAnyDirection')}
      </span>
      {onNewSession ? (
        <button
          type="button"
          className="mt-[10px] inline-flex cursor-pointer items-center gap-[6px] rounded-[var(--r-sm)] border-0 bg-[var(--brand-blue)] px-[16px] py-[8px] text-[12px] font-[600] text-white transition-colors duration-[var(--d1)] hover:bg-[var(--brand-blue-hover)] focus-visible:outline-[2px] focus-visible:outline-offset-2 focus-visible:outline-[var(--focus)]"
          onClick={() => void onNewSession()}
        >
          <Plus size={14} aria-hidden="true" />
          {t('chat:chatDock.newChat')}
          {newSessionShortcut ? (
            <kbd className="ml-[2px] rounded-[4px] border border-white/35 bg-white/10 px-[5px] py-[1px] font-sans text-[10px] font-[500] leading-[1.4]">
              {newSessionShortcut}
            </kbd>
          ) : null}
        </button>
      ) : null}
    </div>
  )
}
