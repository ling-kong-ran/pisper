// 聊天主页面：保留会话目录、实时同步与辅助面板，仅挂载当前活动会话。
// 此 UI 不挂载 Dockview；历史分屏布局不会恢复，其他会话仍可在后台运行。
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import type { DockviewGroupPanel } from 'dockview-react'
import { RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { WorkspacePicker } from '@/components/common/WorkspacePicker'
import { AppEmptyState } from '@/components/ui/app-primitives'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { useClientStore } from '@/stores/client-store'
import { waitForMobileRuntimeReady } from '@/lib/http/http'
import { isPlanActive, resolveSessionPlan } from '@/lib/session/session-state'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { Notify } from '@/app/routes/route-context'
import type { PendingAsset, SessionSummary } from '@/types/chat'
import { SingleSessionPanel } from '@/features/chat/components/ChatDock'
import { chatApi } from '@/features/chat/api/chat-api'
import { ChatDockContext, type ChatDockContextValue } from '@/features/chat/model/chat-dock-context'
import { useChatDock } from '@/features/chat/hooks/use-chat-dock'
import { useLiveSessionSync } from '@/features/chat/hooks/use-live-session-sync'
import { usePromptCommands } from '@/features/chat/hooks/use-prompt-commands'
import { useSessionCatalog } from '@/features/chat/hooks/use-session-catalog'
import { useSessionCommands } from '@/features/chat/hooks/use-session-commands'
import { shouldInheritRecentSessionCwd } from '@/features/chat/model/session-list'
import { updateSessionOrganization } from '@/features/chat/api/session-organization-api'
import {
  SESSION_CREATE_REQUESTED_EVENT,
  consumeSessionCreationRequest,
} from '@/features/chat/model/events'
import { resolveSessionContextPresentation } from '@/features/chat/model/session-context-layout'
import { useSessionContextStore } from '@/features/chat/stores/session-context-store'
import { useSessionContextAutoReveal } from '@/features/chat/hooks/useSessionContextAutoReveal'
import type { SessionContextTab } from '@/features/chat/components/session/SessionContextPanel'
import { SessionContextLayout } from '@/features/chat/components/session/SessionContextLayout'
import { SideChatProvider } from '@/features/chat/components/session/SideChatProvider'
import type { SideChatRuntime } from '@/features/chat/model/side-chat-context'
import { resolveSessionStreaming } from '@/features/chat/model/session-streaming-state'

const LazySessionContextPanel = lazy(() =>
  import('@/features/chat/components/session/SessionContextPanel').then((module) => ({
    default: module.SessionContextPanel,
  })),
)
const SESSION_CONTEXT_PANEL_ID = 'chat-session-context-panel'

function invokeMobile<T>(command: string): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('native bridge unavailable'))
  return invoke<T>(command)
}

type MobileRuntimeState = {
  paired?: boolean
  mode?: 'local' | 'remote' | null
}

type ChatPageProps = {
  notify: Notify
  navigate: (page: string, options?: { replace?: boolean }) => void
  browserNotify?: (event: string, data: unknown, options?: { force?: boolean }) => void
  registerPrimaryAction: (action: () => void) => () => void
  pendingAsset: PendingAsset | null
  onAssetConsumed: () => void
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

export function ChatPage({
  notify,
  navigate,
  browserNotify,
  registerPrimaryAction,
  pendingAsset,
  onAssetConsumed,
  requestText,
  requestConfirm,
}: ChatPageProps) {
  const { t } = useI18n()
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  const clientLoaded = useClientStore((state) => state.loaded)
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const chatLayoutRef = useRef<HTMLDivElement>(null)
  const [contextWidth, setContextWidth] = useState(0)
  const contextOpen = useSessionContextStore((state) => state.open)
  const setContextOpen = useSessionContextStore((state) => state.setOpen)
  const [contextTab, setContextTab] = useState<SessionContextTab>('files')
  useLayoutEffect(() => {
    const layout = chatLayoutRef.current
    if (!layout) return
    const measure = () => setContextWidth(layout.clientWidth)
    measure()
    if (typeof ResizeObserver === 'undefined') {
      window.addEventListener('resize', measure)
      return () => window.removeEventListener('resize', measure)
    }
    const observer = new ResizeObserver(measure)
    observer.observe(layout)
    return () => observer.disconnect()
  }, [])
  const localStreamSessionsRef = useRef(new Set<string>())
  const streamGenerationRef = useRef(new Map<string, number>())
  const resumeSyncRef = useRef<Promise<void> | null>(null)
  const catalog = useSessionCatalog({ notify })
  const markingReadRef = useRef(new Set<string>())
  useEffect(() => {
    const markViewed = () => {
      const id = catalog.activeId
      if (
        !id ||
        document.visibilityState !== 'visible' ||
        !catalog.sessions.some((session) => session.id === id && session.unread) ||
        markingReadRef.current.has(id)
      )
        return
      markingReadRef.current.add(id)
      void updateSessionOrganization(id, { read: true })
        .catch(() => undefined)
        .finally(() => markingReadRef.current.delete(id))
    }
    markViewed()
    document.addEventListener('visibilitychange', markViewed)
    return () => document.removeEventListener('visibilitychange', markViewed)
  }, [catalog.activeId, catalog.sessions])
  const liveSync = useLiveSessionSync({
    sessionStates: catalog.sessionStates,
    sessionStatesRef: catalog.sessionStatesRef,
    localStreamSessionsRef,
    streamGenerationRef,
    updateSessionState: catalog.updateSessionState,
    updateSessions: catalog.updateSessions,
  })
  const dock = useChatDock({
    sessions: catalog.sessions,
    sessionsRef: catalog.sessionsRef,
    sessionStates: catalog.sessionStates,
    activeId: catalog.activeId,
    setActiveId: catalog.setActiveId,
    loading: catalog.loading,
    localStreamSessionsRef,
    loadSessionMessages: liveSync.loadSessionMessages,
    releaseSessionState: catalog.releaseSessionState,
    singleSessionLayout: true,
    notify,
  })
  const activeSession = catalog.sessions.find((session) => session.id === catalog.activeId)
  const activeSessionState = catalog.sessionStates[catalog.activeId]
  const activeStreaming = resolveSessionStreaming(activeSessionState, activeSession)
  const activeCompleted = Boolean(
    activeSessionState?.lifecycle?.phase === 'completed' &&
    !activeSessionState.error &&
    !activeSessionState.runStopped,
  )
  const sessionPlan = resolveSessionPlan(activeSessionState, activeSession)
  const visiblePlan = isPlanActive(sessionPlan, { streaming: activeStreaming }) ? sessionPlan : null
  const contextPresentation = resolveSessionContextPresentation({
    availableWidth: contextWidth,
    mobileLayout,
    hasSession: Boolean(activeSession),
    preference: contextOpen ? 'open' : 'closed',
  })
  const contextCompact = mobileLayout || contextWidth < 800
  useSessionContextAutoReveal({
    sessionId: catalog.activeId,
    streaming: activeStreaming,
    completed: activeCompleted,
    runStartedAt:
      typeof activeSessionState?.runStartedAt === 'string' ? activeSessionState.runStartedAt : null,
    enabled: true,
    open: contextPresentation !== 'closed',
    onReveal: () => {
      setContextTab('files')
      setContextOpen(true)
    },
  })
  const setActiveId = catalog.setActiveId
  const toggleSessionContext = useCallback(
    (sessionId: string, open: boolean) => {
      if (!sessionId) return
      if (open) {
        setActiveId(sessionId)
        setContextOpen(true)
      } else {
        setContextOpen(false)
      }
    },
    [setActiveId, setContextOpen],
  )

  const createSessionRecord = catalog.createSessionRecord
  const loadSessionMessages = liveSync.loadSessionMessages
  const refreshSessions = catalog.refreshSessions
  const sessionStatesRef = catalog.sessionStatesRef
  const syncLiveSession = liveSync.syncLiveSession
  const setGlobalError = catalog.setGlobalError
  const openSessionInDock = dock.openSessionInDock
  const moveSessionToGroup = dock.moveSessionToGroup
  const [recallPulse, setRecallPulse] = useState({ sessionId: '', token: 0 })
  // 新建会话：先创建记录（带可选 cwd），再在 Dock 打开，
  // 若指定了目标分组则把面板移动到该组。
  const createSession = useCallback(
    async (targetGroup?: DockviewGroupPanel, cwd = '') => {
      let mobileState: MobileRuntimeState | null = null
      if (mobileApp) {
        // 手机本机模式不能使用桌面会话路径；只有已配对且明确处于远程模式才继承它。
        mobileState = await invokeMobile<MobileRuntimeState>('mobile_state').catch(() => null)
      }
      const inheritRecentCwd = shouldInheritRecentSessionCwd(mobileApp, mobileState)
      const sessionId = await createSessionRecord(cwd, { inheritRecentCwd })
      if (!sessionId) return ''
      const opened = openSessionInDock(sessionId)
      if (opened) moveSessionToGroup(sessionId, targetGroup)
      return sessionId
    },
    [createSessionRecord, mobileApp, moveSessionToGroup, openSessionInDock],
  )
  usePagePrimaryAction(registerPrimaryAction, createSession)

  const promptCommands = usePromptCommands({
    browserNotify,
    notify,
    defaultModel: catalog.defaultModel,
    localStreamSessionsRef,
    streamGenerationRef,
    sessionStatesRef: catalog.sessionStatesRef,
    setActiveId: catalog.setActiveId,
    setGlobalError: catalog.setGlobalError,
    updateSessionState: catalog.updateSessionState,
    updateSessions: catalog.updateSessions,
    createSession,
    loadSessionMessages: liveSync.loadSessionMessages,
    refreshSessions: catalog.refreshSessions,
    syncLiveSession: liveSync.syncLiveSession,
  })
  const { sendPrompt, retryLastTurn } = promptCommands

  // 处理会话创建请求（可携带自动发送的提示词，如视觉生成卡片的「试试示例」）：
  // 事件监听 + localStorage 持久化，跨页面跳转/重启后挂载时也会补建。
  useEffect(() => {
    const createRequested = () => {
      const request = consumeSessionCreationRequest()
      if (!request) return
      void (async () => {
        const sessionId = await createSession(undefined, request.cwd)
        if (sessionId && request.prompt) void sendPrompt(request.prompt, sessionId)
      })()
    }
    window.addEventListener(SESSION_CREATE_REQUESTED_EVENT, createRequested)
    createRequested()
    return () => window.removeEventListener(SESSION_CREATE_REQUESTED_EVENT, createRequested)
  }, [createSession, sendPrompt])
  const sessionCommands = useSessionCommands({
    notify,
    requestText,
    requestConfirm,
    availableModels: catalog.availableModels,
    sessionStatesRef: catalog.sessionStatesRef,
    updateSessionState: catalog.updateSessionState,
    updateSessions: catalog.updateSessions,
    replaceSessionStates: catalog.replaceSessionStates,
    setGlobalError: catalog.setGlobalError,
    syncLiveSession: liveSync.syncLiveSession,
  })

  // 移动 WebView 从后台恢复时，SSE 可能既不报错也不再产生活动；
  // 先刷新目录，再强制用服务端快照校准所有已缓存/仍运行的会话。
  const syncAfterForeground = useCallback(() => {
    if (document.visibilityState !== 'visible' || resumeSyncRef.current) return
    const request = (async () => {
      if (mobileApp) {
        // 与 API 共用有界恢复闸门，避免独立原生命令挂起后永久锁住 resumeSyncRef。
        await waitForMobileRuntimeReady().catch(() => undefined)
      }
      let sessions: SessionSummary[] = []
      try {
        sessions = await refreshSessions(undefined, { preserveExistingOnEmpty: true })
      } catch {
        // 实时快照仍可使用已缓存的会话状态，目录请求失败不阻断恢复。
      }
      const ids = new Set([
        ...Object.keys(sessionStatesRef.current),
        ...(catalog.activeId ? [catalog.activeId] : []),
        ...sessions.filter((session) => session.streaming).map((session) => session.id),
      ])
      await Promise.allSettled(Array.from(ids).map((id) => syncLiveSession(id, { force: true })))
    })().finally(() => {
      resumeSyncRef.current = null
    })
    resumeSyncRef.current = request
  }, [catalog.activeId, mobileApp, refreshSessions, sessionStatesRef, syncLiveSession])

  useEffect(() => {
    const recover = () => syncAfterForeground()
    document.addEventListener('visibilitychange', recover)
    window.addEventListener('pageshow', recover)
    window.addEventListener('online', recover)
    return () => {
      document.removeEventListener('visibilitychange', recover)
      window.removeEventListener('pageshow', recover)
      window.removeEventListener('online', recover)
    }
  }, [syncAfterForeground])

  // 强制重载会话分支：刷新消息 + 列表，供恢复/切换后同步。
  const reloadSessionBranch = useCallback(
    async (sessionId: string) => {
      await loadSessionMessages(sessionId, { force: true })
      await refreshSessions(sessionId)
    },
    [loadSessionMessages, refreshSessions],
  )

  // 从边界条目派生新分支：调用运行时导航到历史条目处分支，
  // 成功后刷新列表并在 Dock 打开新会话。
  const branchFromEntry = useCallback(
    async (session: SessionSummary, boundaryEntryId: string) => {
      if (!session?.id || !boundaryEntryId) return
      try {
        setGlobalError('')
        const result = await chatApi.navigateSessionTree(session.id, boundaryEntryId, false)
        if (result.cancelled) return
        await reloadSessionBranch(session.id)
        setRecallPulse((current) => ({ sessionId: session.id, token: current.token + 1 }))
        notify(t('chat:chatPage.branchedFromNode'))
      } catch (error) {
        setGlobalError(error instanceof Error ? error.message : String(error))
      }
    },
    [notify, reloadSessionBranch, setGlobalError, t],
  )

  // 从已完成回复创建独立对话：命名后调用运行时 derive 接口，
  // 新会话拥有独立上下文，创建成功后立即打开对应 Dock 面板。
  const createChildSession = useCallback(
    async (session: SessionSummary, boundaryEntryId: string) => {
      if (!session?.id || !boundaryEntryId) return
      const name = await requestText({
        title: t('chat:chatPage.createChildChat'),
        inputLabel: t('chat:chatPage.chatTitle'),
        value: `${t('chat:chatPage.separateChatSuffix')} · ${session.name || t('chat:chatPage.newChat')}`,
        confirmLabel: t('chat:chatPage.create'),
      })
      if (name === null) return
      try {
        setGlobalError('')
        const created = await chatApi.deriveSession(session.id, boundaryEntryId, name)
        // 目录刷新不抢先选中子会话，只由 Dock 执行一次切换。延时再次打开会覆盖
        // 用户紧接着发起的「返回原对话」；动画不能拥有会话选择状态。
        await refreshSessions()
        openSessionInDock(created.id)
        setRecallPulse((current) => ({ sessionId: created.id, token: current.token + 1 }))
        notify(t('chat:chatPage.childChatCreated'))
      } catch (error) {
        setGlobalError(error instanceof Error ? error.message : String(error))
      }
    },
    [notify, openSessionInDock, refreshSessions, requestText, setGlobalError, t],
  )

  const openModelSettings = useCallback(() => navigate('config'), [navigate])
  // 会话状态不进 context：流式期间 sessionStates 每帧变化，
  // 若随 context 广播会让所有 Dock 面板每帧重渲染。面板改为按会话订阅，
  // context 只携带低频数据与稳定回调，配合 useMemo 保持引用稳定。
  const dockContextValue: ChatDockContextValue = useMemo(
    () => ({
      sessions: catalog.sessions,
      subscribeSessionState: catalog.subscribeSessionState,
      getSessionState: catalog.getSessionState,
      defaultModel: catalog.defaultModel,
      availableModels: catalog.availableModels,
      globalError: catalog.globalError,
      activeId: catalog.activeId,
      compactDock: dock.compactDock,
      contextTab: contextPresentation === 'closed' ? null : contextTab,
      contextCompact,
      contextPanelId: SESSION_CONTEXT_PANEL_ID,
      toggleSessionContext,
      sessionTreePulseSessionId: recallPulse.sessionId,
      sessionTreePulseToken: recallPulse.token,
      pendingAsset,
      onAssetConsumed,
      notify,
      openModelSettings,
      loadSessionMessages: liveSync.loadSessionMessages,
      loadOlderMessages: liveSync.loadOlderMessages,
      sendPrompt: promptCommands.sendPrompt,
      retryLastTurn: promptCommands.retryLastTurn,
      queuePrompt: promptCommands.queuePrompt,
      withdrawQueuedInput: promptCommands.withdrawQueuedInput,
      abort: promptCommands.abort,
      pauseGoal: sessionCommands.pauseGoal,
      setGoalBudget: sessionCommands.setGoalBudget,
      compactSession: sessionCommands.compactSession,
      setCompactionThreshold: sessionCommands.setCompactionThreshold,
      switchSessionModel: sessionCommands.switchSessionModel,
      loadSessionThinkingLevel: sessionCommands.loadSessionThinkingLevel,
      switchSessionThinkingLevel: sessionCommands.switchSessionThinkingLevel,
      switchSessionExecutionMode: sessionCommands.switchSessionExecutionMode,
      switchSessionRunMode: sessionCommands.switchSessionRunMode,
      resolveToolApproval: sessionCommands.resolveToolApproval,
      selectSessionWorkspace: sessionCommands.selectSessionWorkspace,
      renameSession: sessionCommands.renameSession,
      branchFromEntry,
      createChildSession,
      reloadSessionBranch,
      splitDockPanel: dock.splitDockPanel,
      closeDockPanel: dock.closeDockPanel,
    }),
    [
      catalog.sessions,
      catalog.subscribeSessionState,
      catalog.getSessionState,
      catalog.defaultModel,
      catalog.availableModels,
      catalog.globalError,
      catalog.activeId,
      dock.compactDock,
      contextPresentation,
      contextTab,
      contextCompact,
      toggleSessionContext,
      dock.splitDockPanel,
      dock.closeDockPanel,
      recallPulse.sessionId,
      recallPulse.token,
      pendingAsset,
      onAssetConsumed,
      notify,
      openModelSettings,
      liveSync.loadSessionMessages,
      liveSync.loadOlderMessages,
      promptCommands.sendPrompt,
      promptCommands.retryLastTurn,
      promptCommands.queuePrompt,
      promptCommands.withdrawQueuedInput,
      promptCommands.abort,
      sessionCommands.pauseGoal,
      sessionCommands.setGoalBudget,
      sessionCommands.compactSession,
      sessionCommands.setCompactionThreshold,
      sessionCommands.switchSessionModel,
      sessionCommands.loadSessionThinkingLevel,
      sessionCommands.switchSessionThinkingLevel,
      sessionCommands.switchSessionExecutionMode,
      sessionCommands.switchSessionRunMode,
      sessionCommands.resolveToolApproval,
      sessionCommands.selectSessionWorkspace,
      sessionCommands.renameSession,
      branchFromEntry,
      createChildSession,
      reloadSessionBranch,
    ],
  )

  const discardSessionState = catalog.discardSessionState
  const sideChatRuntime: SideChatRuntime = useMemo(
    () => ({
      subscribeSessionState: catalog.subscribeSessionState,
      getSessionState: catalog.getSessionState,
      retainSessionState: catalog.retainSessionState,
      discard: (id) => {
        // 只在服务端确认旧临时 ID 已失效时废弃本地流，不向后台发送 abort。
        streamGenerationRef.current.set(id, (streamGenerationRef.current.get(id) ?? 0) + 1)
        localStreamSessionsRef.current.delete(id)
        discardSessionState(id)
      },
      loadSessionMessages: liveSync.loadSessionMessages,
      loadOlderMessages: liveSync.loadOlderMessages,
      syncLiveSession: liveSync.syncLiveSession,
      send: (id, text) => sendPrompt(text, id, [], false, false, null, null, { activate: false }),
      retry: (id) => retryLastTurn(id, { activate: false }),
      abort: promptCommands.abort,
      approve: sessionCommands.resolveToolApproval,
    }),
    [
      catalog.subscribeSessionState,
      catalog.getSessionState,
      catalog.retainSessionState,
      discardSessionState,
      liveSync.loadSessionMessages,
      liveSync.loadOlderMessages,
      liveSync.syncLiveSession,
      sendPrompt,
      retryLastTurn,
      promptCommands.abort,
      sessionCommands.resolveToolApproval,
    ],
  )

  const contextPanel = activeSession && contextPresentation !== 'closed' && (
    <Suspense
      fallback={
        contextPresentation === 'aside' ? (
          <aside
            className="h-full min-h-0 w-full rounded-[var(--r-md)] border border-[var(--stroke-soft)] bg-[var(--panel)] p-4 text-sm text-[var(--text-muted)]"
            role="status"
          >
            {t('chat:focusSession.gitLoading')}
          </aside>
        ) : null
      }
    >
      <LazySessionContextPanel
        key={activeSession.id}
        panelId={SESSION_CONTEXT_PANEL_ID}
        compact={contextPresentation === 'sheet'}
        sessionId={activeSession.id}
        tab={contextTab}
        plan={runtimeFeatureAvailable(capabilities, 'plans') ? visiblePlan : null}
        streaming={activeStreaming}
        plansAvailable={runtimeFeatureAvailable(capabilities, 'plans')}
        requestConfirm={requestConfirm}
        onTabChange={setContextTab}
        onClose={() => setContextOpen(false)}
      />
    </Suspense>
  )

  return (
    <SideChatProvider runtime={sideChatRuntime}>
      <div
        ref={chatLayoutRef}
        className="chat-layout dock-layout relative flex w-full min-w-0 min-h-0 flex-1"
      >
        <SessionContextLayout
          availableWidth={contextWidth}
          presentation={contextPresentation}
          context={contextPanel}
          side="right"
        >
          {catalog.loading ? (
            <AppEmptyState>
              <RefreshCw className="animate-spin" size={24} />
              <h2>{t('chat:chatPage.wakingTheAgent')}</h2>
              <p>{t('chat:chatPage.modelsSessionsAndContextAreSettlingIntoPlace')}</p>
            </AppEmptyState>
          ) : (
            <div className="chat-dock-workspace relative h-full w-full min-w-0 min-h-0 flex-1 [isolation:isolate] overflow-hidden border-0 rounded-none bg-background">
              <ChatDockContext.Provider value={dockContextValue}>
                {clientLoaded && <SingleSessionPanel onCreateSession={createSession} />}
              </ChatDockContext.Provider>
            </div>
          )}
        </SessionContextLayout>
      </div>
      {sessionCommands.workspaceSession && (
        <WorkspacePicker
          open
          initialPath={sessionCommands.workspaceSession.cwd}
          description={t('common:workspacePicker.selectWorkspaceForChat', {
            name: sessionCommands.workspaceSession.name,
          })}
          onOpenChange={(open) => !open && sessionCommands.setWorkspaceSession(null)}
          onSelect={(cwd) =>
            sessionCommands.switchSessionCwd(sessionCommands.workspaceSession!, cwd)
          }
        />
      )}
    </SideChatProvider>
  )
}
