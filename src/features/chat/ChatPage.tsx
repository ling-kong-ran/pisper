// 聊天主页面持有会话目录与实时连接，单会话视图在这里装配。
// 会话页（/board/:sessionId）以路由模式嵌入：只挂载 URL 指定的单个会话，
// 任何内部打开会话的动作都交回路由，再由 URL 回写活动会话。
// 第 3 期起 Dockview 多标签与分屏不再挂载（代码留到清理期删除）：单会话视图是唯一形态。
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { useClientStore } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { waitForMobileRuntimeReady } from '@/lib/http'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { Notify } from '@/app/route-context'
import type { PendingAsset, SessionSummary } from '@/types/chat'
import { SingleSessionPanel } from './ChatDock'
import { chatApi } from './chat-api'
import { ChatDockContext, type ChatDockContextValue } from './chat-dock-context'
import { useSessionSelection } from './use-session-selection'
import { useLiveSessionSync } from './use-live-session-sync'
import { usePromptCommands } from './use-prompt-commands'
import { useSessionCatalog } from './use-session-catalog'
import { useSessionCommands } from './use-session-commands'
import { shouldInheritRecentSessionCwd } from './session-list'
import { updateSessionOrganization } from './session-organization-api'
import {
  SESSION_CREATE_REQUESTED_EVENT,
  consumeSessionCreationRequest,
  consumeSessionSelectionRequest,
  subscribeSessionBranchChanges,
} from './events'
import { SessionRouteLeading } from './session-route-chrome'
import { SessionPageChromeContext, type SessionPageChrome } from './session-workspace-context'
import { openWebPreviewInSessionContext } from './session-web-preview'
import { WEB_PREVIEW_OPEN_EVENT, consumeWebPreviewRequest } from './web-preview-events'
import { SideChatProvider } from './SideChatProvider'
import { notifyTreeNavigation } from './session-tree-actions'
import { applySessionTreeResponse } from './session-tree-summary'
import type { SideChatRuntime } from './side-chat-context'

// 会话页路由里的保留段：进入后新建会话，再以 replace 换成真实 id（与 '/workflows/new' 同理）。
const NEW_SESSION_ROUTE_ID = 'new'
const FOCUS_MESSAGE_PAGE_SIZE = 40

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
  // 路由模式：URL 里的会话 id（或 'new'）是唯一选中依据；内部打开会话时回调路由切换。
  routeSessionId?: string
  onRouteSessionChange?: (sessionId: string) => void
  // 会话页注入到会话顶栏最左侧的返回入口（路由模式且手机外壳才有；桌面返回键在泳道条上）。
  routeLeading?: ReactNode
  // 会话页桌面布局（页头 44 + 对话 + 检查器）；手机外壳不传，保持原来的单会话视图。
  pageChrome?: SessionPageChrome
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
  routeSessionId,
  onRouteSessionChange,
  routeLeading,
  pageChrome,
}: ChatPageProps) {
  const { t } = useI18n()
  const routeMode = routeSessionId !== undefined
  // 'new' 只是“正在新建”的占位，不是可选中的会话 id。
  const routeTarget = routeMode && routeSessionId !== NEW_SESSION_ROUTE_ID ? routeSessionId : ''
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  const clientLoaded = useClientStore((state) => state.loaded)
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  const chatLayoutRef = useRef<HTMLDivElement>(null)
  const localStreamSessionsRef = useRef(new Set<string>())
  const streamGenerationRef = useRef(new Map<string, number>())
  const resumeSyncRef = useRef<Promise<void> | null>(null)
  const catalog = useSessionCatalog({ notify })
  const markingReadRef = useRef(new Set<string>())
  // 每个会话的这一段“未读”只自动标记一次：标记失败或服务端仍回 unread 时，
  // 不能因为目录刷新又触发标记——那会和目录刷新互相触发，形成每秒几十次的请求风暴。
  // 会话回到已读后清掉记录；页面重新可见时允许再试一次。
  const markedReadRef = useRef(new Set<string>())
  useEffect(() => {
    for (const id of markedReadRef.current) {
      if (!catalog.sessions.some((session) => session.id === id && session.unread))
        markedReadRef.current.delete(id)
    }
    const markViewed = (event?: Event) => {
      const id = catalog.activeId
      if (
        !id ||
        document.visibilityState !== 'visible' ||
        !catalog.sessions.some((session) => session.id === id && session.unread) ||
        markingReadRef.current.has(id) ||
        (!event && markedReadRef.current.has(id))
      )
        return
      markingReadRef.current.add(id)
      markedReadRef.current.add(id)
      void updateSessionOrganization(id, { read: true })
        .catch(() => undefined)
        .finally(() => markingReadRef.current.delete(id))
    }
    markViewed()
    document.addEventListener('visibilitychange', markViewed)
    return () => document.removeEventListener('visibilitychange', markViewed)
  }, [catalog.activeId, catalog.sessions])
  // 看板事件流在线时，非当前会话的运行状态由看板 store 维护：/live 只为当前会话轮询。
  const boardEvents = useRuntimeCapabilitiesStore(
    (state) => state.capabilities.features['board.events'] === true,
  )
  const liveSync = useLiveSessionSync({
    sessionStates: catalog.sessionStates,
    sessionStatesRef: catalog.sessionStatesRef,
    localStreamSessionsRef,
    streamGenerationRef,
    updateSessionState: catalog.updateSessionState,
    updateSessions: catalog.updateSessions,
    pollOnly: boardEvents ? catalog.activeId || null : undefined,
  })
  const selection = useSessionSelection({
    setActiveId: catalog.setActiveId,
    loadSessionMessages: liveSync.loadSessionMessages,
    openRoutedSession: routeMode ? onRouteSessionChange : undefined,
  })
  const createSessionRecord = catalog.createSessionRecord
  const loadSessionMessages = liveSync.loadSessionMessages
  const refreshSessions = catalog.refreshSessions
  const sessionStatesRef = catalog.sessionStatesRef
  const syncLiveSession = liveSync.syncLiveSession
  const setGlobalError = catalog.setGlobalError
  const openSession = selection.openSession
  const [recallPulse, setRecallPulse] = useState({ sessionId: '', token: 0 })
  // 新建会话：先创建记录（带可选 cwd），再打开它（会话页交给路由）。
  const createSession = useCallback(
    async (cwd = '') => {
      let mobileState: MobileRuntimeState | null = null
      if (mobileApp) {
        // 手机本机模式不能使用桌面会话路径；只有已配对且明确处于远程模式才继承它。
        mobileState = await invokeMobile<MobileRuntimeState>('mobile_state').catch(() => null)
      }
      const inheritRecentCwd = shouldInheritRecentSessionCwd(mobileApp, mobileState)
      const sessionId = await createSessionRecord(cwd, { inheritRecentCwd })
      if (!sessionId) return ''
      openSession(sessionId)
      return sessionId
    },
    [createSessionRecord, mobileApp, openSession],
  )
  usePagePrimaryAction(registerPrimaryAction, createSession)

  // 路由模式：URL → 活动会话。目录刷新或删除可能把活动会话换走，只要路由会话仍在目录里就拉回；
  // 路由会话不在目录里（已删除或链接错误）时不强选，由页面显示“找不到这个会话”。
  const setActiveId = catalog.setActiveId
  const routeKnown = Boolean(
    routeTarget && catalog.sessions.some((session) => session.id === routeTarget),
  )
  useEffect(() => {
    if (!routeTarget || catalog.activeId === routeTarget) return
    if (!routeKnown && !catalog.loading) return
    setActiveId(routeTarget)
  }, [catalog.activeId, catalog.loading, routeKnown, routeTarget, setActiveId])

  // 进入会话页时消费遗留的打开请求：命中当前会话且带消息定位时强制重载，让定位条目可见；
  // 指向别的会话的旧请求直接丢弃，地址栏才是选中依据。
  const routeEntryHandledRef = useRef('')
  useEffect(() => {
    if (!routeTarget || routeEntryHandledRef.current === routeTarget) return
    routeEntryHandledRef.current = routeTarget
    const request = consumeSessionSelectionRequest()
    if (request?.sessionId === routeTarget && request.targetEntryId) {
      void loadSessionMessages(routeTarget, { force: true, limit: FOCUS_MESSAGE_PAGE_SIZE })
    }
  }, [loadSessionMessages, routeTarget])

  // 会话页没有 Dock 预览面板：外链预览落到当前会话检查器的网页预览页；
  // 从别的页面带过来的预览请求在进入会话页时补开。只在路由会话确认存在后消费：
  // 目录还在加载或会话已删除（“找不到这个会话”）时请求留着，不被白白吃掉。
  useEffect(() => {
    if (!routeTarget || !routeKnown) return
    const pending = consumeWebPreviewRequest()
    if (pending) openWebPreviewInSessionContext(routeTarget, pending.url)
    const openPreview = (event: Event) => {
      const url = (event as CustomEvent<{ url?: string }>).detail?.url
      if (url) openWebPreviewInSessionContext(routeTarget, url)
    }
    window.addEventListener(WEB_PREVIEW_OPEN_EVENT, openPreview)
    return () => window.removeEventListener(WEB_PREVIEW_OPEN_EVENT, openPreview)
  }, [routeKnown, routeTarget])

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
  // 路由停在 '/board/new' 且没有待处理的创建请求时（直接打开或刷新该地址），新建一个空会话；
  // 每次进入 'new' 只建一次，创建完成后路由以 replace 换成真实 id。
  // 会话页等目录首载完成再建：否则首载快照会覆盖刚合并的新会话，页面误报“找不到这个会话”。
  const newRouteHandledRef = useRef(false)
  const awaitingNewRoute = routeMode && routeSessionId === NEW_SESSION_ROUTE_ID
  const routeCatalogLoading = routeMode && catalog.loading
  useEffect(() => {
    if (!awaitingNewRoute) newRouteHandledRef.current = false
  }, [awaitingNewRoute])
  useEffect(() => {
    if (routeCatalogLoading) return
    const consumeCreateRequest = () => {
      const request = consumeSessionCreationRequest()
      if (!request) return false
      if (awaitingNewRoute) newRouteHandledRef.current = true
      void (async () => {
        const sessionId = await createSession(request.cwd)
        if (sessionId && request.prompt) void sendPrompt(request.prompt, sessionId)
      })()
      return true
    }
    const createRequested = () => void consumeCreateRequest()
    window.addEventListener(SESSION_CREATE_REQUESTED_EVENT, createRequested)
    if (!consumeCreateRequest() && awaitingNewRoute && !newRouteHandledRef.current) {
      newRouteHandledRef.current = true
      void createSession()
    }
    return () => window.removeEventListener(SESSION_CREATE_REQUESTED_EVENT, createRequested)
  }, [awaitingNewRoute, createSession, routeCatalogLoading, sendPrompt])
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
        // 响应里带着新树：顶栏分支数与回合标记立刻更新（老问题 6），不等会话树事件。
        applySessionTreeResponse(session.id, result)
        await reloadSessionBranch(session.id)
        setRecallPulse((current) => ({ sessionId: session.id, token: current.token + 1 }))
        // 提示里带“撤销”（老问题 3）：把 Runtime 给的 restoreEntryId 传回导航接口。
        notifyTreeNavigation(session.id, result, t('chat:chatPage.branchedFromNode'), { notify, t })
      } catch (error) {
        setGlobalError(error instanceof Error ? error.message : String(error))
      }
    },
    [notify, reloadSessionBranch, setGlobalError, t],
  )

  // 活动分支在别处被切换（撤销、⌘K / 会话树里打开另一条分支上的标记）：已加载的会话重读消息与列表。
  useEffect(
    () =>
      subscribeSessionBranchChanges((id) => {
        if (!sessionStatesRef.current[id]) return
        void reloadSessionBranch(id).catch(() => undefined)
      }),
    [reloadSessionBranch, sessionStatesRef],
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
        // 目录刷新不抢先选中子会话，只在这里切换一次。延时再次打开会覆盖
        // 用户紧接着发起的「返回原对话」；动画不能拥有会话选择状态。
        await refreshSessions()
        openSession(created.id)
        setRecallPulse((current) => ({ sessionId: created.id, token: current.token + 1 }))
        notify(t('chat:chatPage.childChatCreated'))
      } catch (error) {
        setGlobalError(error instanceof Error ? error.message : String(error))
      }
    },
    [notify, openSession, refreshSessions, requestText, setGlobalError, t],
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
      mobileLayout,
      requestConfirm,
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
    }),
    [
      catalog.sessions,
      catalog.subscribeSessionState,
      catalog.getSessionState,
      catalog.defaultModel,
      catalog.availableModels,
      catalog.globalError,
      catalog.activeId,
      mobileLayout,
      requestConfirm,
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
      send: async (id, text) => {
        await sendPrompt(text, id, [], false, false, null, null, { activate: false })
      },
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

  return (
    <SideChatProvider runtime={sideChatRuntime}>
      <div
        ref={chatLayoutRef}
        className="chat-layout dock-layout relative flex w-full min-w-0 min-h-0 flex-1"
      >
        {catalog.loading ? (
          // 首次唤醒沿用品牌轨道指示，不画额外卡片框，让纸面保持安静。
          <PageState
            kind="loading"
            title={t('chat:chatPage.wakingTheAgent')}
            description={t('chat:chatPage.modelsSessionsAndContextAreSettlingIntoPlace')}
          />
        ) : (
          <div className="chat-dock-workspace relative h-full w-full min-w-0 min-h-0 flex-1 [isolation:isolate] overflow-hidden border-0 rounded-none bg-panel">
            <ChatDockContext.Provider value={dockContextValue}>
              <SessionPageChromeContext.Provider value={routeMode ? (pageChrome ?? null) : null}>
                <SessionRouteLeading.Provider value={routeMode ? (routeLeading ?? null) : null}>
                  {clientLoaded &&
                    (routeMode ? (
                      routeTarget && !routeKnown ? (
                        <PageState
                          kind="empty"
                          title={t('navigation:shell.sessionNotFound')}
                          description={t('navigation:shell.sessionNotFoundHint')}
                          action={
                            pageChrome?.onHome ? (
                              <Button variant="secondary" onClick={pageChrome.onHome}>
                                {t('board:home.goHome')}
                              </Button>
                            ) : (
                              <Button variant="secondary" onClick={() => navigate('board')}>
                                {t('navigation:shell.backToBoardLabel')}
                              </Button>
                            )
                          }
                        />
                      ) : routeTarget ? (
                        <SingleSessionPanel
                          sessionId={routeTarget}
                          onCreateSession={createSession}
                        />
                      ) : catalog.globalError ? (
                        <PageState
                          kind="error"
                          title={t('navigation:shell.createSessionFailed')}
                          description={catalog.globalError}
                          action={
                            <Button variant="secondary" onClick={() => void createSession()}>
                              {t('navigation:more.retry')}
                            </Button>
                          }
                        />
                      ) : (
                        <PageState kind="loading" title={t('chat:chatPage.wakingTheAgent')} />
                      )
                    ) : (
                      <SingleSessionPanel onCreateSession={createSession} />
                    ))}
                </SessionRouteLeading.Provider>
              </SessionPageChromeContext.Provider>
            </ChatDockContext.Provider>
          </div>
        )}
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
