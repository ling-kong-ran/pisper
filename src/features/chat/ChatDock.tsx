import { TASK_VIEW_WILL_CHANGE } from './events'
// 聊天 Dock 面板容器：每个 dockview 面板对应一个会话视图，
// 负责把面板事件（关闭/激活）桥接到会话状态与布局持久化。
import { useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import type { IDockviewPanelProps } from 'dockview-react'
import { AlertTriangle, History, MessageSquare, Plus, X } from 'lucide-react'
import { StarOrbit } from '@/components/StarOrbit'
import { Button } from '@/components/ui/button'
import { Kbd, KbdGroup } from '@/components/ui/kbd'
import { OrbitSpinner } from '@/components/ui/spinner'
import { STORAGE_KEYS } from '@/app/storage'
import { pageStateStorage } from '@/lib/page-state-storage'
import { cn } from '@/lib/utils'
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
import { closeMobileSessionTab, sessionIdFromPanel } from './dock-layout'

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
  const activeId = context?.activeId || ''
  if (!activeId) return <ChatDockWatermark onNewSession={onCreateSession} />
  return (
    <SessionPanel
      key={activeId}
      sessionId={activeId}
      panelId={`single-${activeId}`}
      shortcutEnabled
      canSplitPanel={false}
      canClosePanel={false}
    />
  )
}

type MobileSessionPanelProps = {
  sessionIds?: string[]
  onSelectSession: (sessionId: string) => void
  onCreateSession: () => void | Promise<unknown>
  onOpenHistory: () => void
}

function readMobileSessionTabs() {
  try {
    const value: unknown = JSON.parse(
      pageStateStorage.getItem(STORAGE_KEYS.mobileSessionTabs) || '[]',
    )
    return Array.isArray(value)
      ? value.filter((item): item is string => typeof item === 'string')
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
  onOpenHistory,
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
  // 手机外壳只有一个会话时，标签条只会重复下方顶栏的标题；新会话与历史仍在菜单抽屉里。
  // 桌面壳窄窗口保留标签条，不改变其顶栏与窗口按钮的让位关系。
  const showTabs = Boolean(window.pisperDesktop) || visibleTabs.length > 1

  useEffect(() => {
    const knownIds = new Set(context?.sessions.map((session) => session.id))
    const seedIds = !seededSessionIdsRef.current && sessionIds.length ? sessionIds : []
    if (seedIds.length) seededSessionIdsRef.current = true
    setTabIds((current) => {
      const next = current.filter((id) => knownIds.has(id))
      for (const id of [...seedIds, activeId]) {
        if (id && knownIds.has(id) && !next.includes(id)) next.push(id)
      }
      const limited = next
      return limited.length === current.length &&
        limited.every((id, index) => id === current[index])
        ? current
        : limited
    })
  }, [activeId, context?.sessions, sessionIds])

  useEffect(() => {
    pageStateStorage.setItem(STORAGE_KEYS.mobileSessionTabs, JSON.stringify(tabIds))
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
    <div className="mobile-session-shell flex h-full min-h-0 min-w-0 flex-col bg-panel">
      {/* 浮动侧栏开关压在首行左侧；标签条让出该宽度，会话内容区不再重复让位。
          移动顶栏与纸面同色，只用底部细线分层；选中会话以 2px 强调刻度标出，运行中显示轨道指示。 */}
      {showTabs && (
        <div className="mobile-session-tabs flex h-[52px] flex-none border-b border-stroke-soft bg-panel pl-[calc(var(--workbench-leading-inset,0px)+4px)]">
          <button
            type="button"
            className="flex min-h-11 flex-none items-center gap-1.5 border-0 border-r border-stroke-soft bg-transparent px-3 text-xs font-medium text-content-secondary hover:bg-surface-hover hover:text-content focus-visible:z-[1] focus-visible:-outline-offset-2"
            title={t('navigation:appSidebar.viewAllCountChats', {
              count: context?.sessions.length || 0,
            })}
            aria-label={t('navigation:appSidebar.viewAllCountChats', {
              count: context?.sessions.length || 0,
            })}
            onClick={onOpenHistory}
          >
            <History size={16} aria-hidden="true" />
            <span>{t('navigation:appSidebar.viewAll')}</span>
          </button>
          <div
            className="flex min-w-0 flex-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
            role="tablist"
            aria-label={t('chat:chatPage.openChats')}
          >
            {visibleTabs.map((session) => {
              const title = session.name || t('chat:chatPage.untitledChat')
              const active = session.id === sessionId
              return (
                <div
                  className={`relative flex h-full min-w-[148px] max-w-[228px] flex-none items-stretch transition-colors duration-(--d-fast) ${active ? 'font-medium text-content after:absolute after:inset-x-3 after:bottom-0 after:h-0.5 after:rounded-full after:bg-star' : 'text-content-muted hover:bg-surface-hover hover:text-content'}`}
                  key={session.id}
                >
                  <button
                    ref={active ? activeTabRef : undefined}
                    type="button"
                    role="tab"
                    id={`mobile-session-tab-${session.id}`}
                    aria-selected={active}
                    aria-controls="mobile-session-tabpanel"
                    className="flex min-w-0 flex-1 items-center gap-2 border-0 bg-transparent py-0 pl-3 text-left text-base focus-visible:z-[1] focus-visible:-outline-offset-2"
                    title={title}
                    onClick={() => onSelectSession(session.id)}
                  >
                    {session.streaming ? (
                      <OrbitSpinner className="size-4" />
                    ) : (
                      <MessageSquare size={16} className="flex-none" aria-hidden="true" />
                    )}
                    <span className="min-w-0 flex-1 truncate">{title}</span>
                    {session.streaming && (
                      <span className="sr-only">{t('chat:focusSession.running')}</span>
                    )}
                  </button>
                  <button
                    type="button"
                    className="grid min-h-11 w-11 flex-none place-items-center border-0 bg-transparent text-content-muted hover:bg-surface-hover hover:text-content focus-visible:z-[1] focus-visible:-outline-offset-2"
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
            className="grid h-full w-[52px] flex-none place-items-center border-0 border-l border-stroke-soft bg-transparent text-content-secondary hover:bg-surface-hover hover:text-content focus-visible:-outline-offset-2"
            title={t('chat:chatPage.newChat')}
            aria-label={t('chat:chatPage.newChat')}
            onClick={() => void onCreateSession()}
          >
            <Plus size={22} aria-hidden="true" />
          </button>
        </div>
      )}
      {/* 无标签条时首行就是会话顶栏，由它为浮动菜单键让位；也不再声明孤立的 tabpanel。 */}
      <div
        className={cn('min-h-0 min-w-0 flex-1', showTabs && '[--workbench-leading-inset:0px]')}
        role={showTabs ? 'tabpanel' : undefined}
        id="mobile-session-tabpanel"
        aria-labelledby={showTabs && sessionId ? `mobile-session-tab-${sessionId}` : undefined}
      >
        {sessionId ? (
          <SessionPanel
            sessionId={sessionId}
            panelId=""
            shortcutEnabled
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
  return (
    <div
      className="session-dock-panel w-full h-full min-w-0 min-h-0 overflow-hidden [container-type:inline-size] bg-panel"
      onFocusCapture={onFocusCapture}
    >
      <TaskWorkspace
        messages={state.historyWindow?.messages || state.messages || EMPTY_LIST}
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
          messages={state.historyWindow?.messages || state.messages || EMPTY_LIST}
          transcriptLoadState={
            state.loaded ? 'ready' : state.loading || !state.error ? 'loading' : 'error'
          }
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
