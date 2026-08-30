// 聊天主页面：dockview 多会话分屏布局的宿主，持有会话目录与实时
// 同步状态，管理 Dock 的初始化/持久化与多面板交互。
// 移动端 App 不渲染 Dock：轻量标签栏切换活动会话，内容区只挂载一个会话，
// dockview 及其样式经懒加载分包，移动端不下载。
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { DockviewGroupPanel } from 'dockview-react'
import { RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { AppEmptyState } from '@/components/ui/app-primitives'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { useClientStore } from '@/stores/client-store'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { Notify } from '@/app/route-context'
import type { PendingAsset, SessionSummary } from '@/types/chat'
import { MobileSessionPanel } from './ChatDock'
import { chatApi } from './chat-api'
import { ChatDockContext, type ChatDockContextValue } from './chat-dock-context'
import { useChatDock } from './use-chat-dock'
import { useLiveSessionSync } from './use-live-session-sync'
import { usePromptCommands } from './use-prompt-commands'
import { useSessionCatalog } from './use-session-catalog'
import { useSessionCommands } from './use-session-commands'
import { shouldInheritRecentSessionCwd } from './session-list'
import { SESSION_CREATE_REQUESTED_EVENT, consumeSessionCreationRequest } from './events'

// Dock 分屏视图懒加载：只有桌面布局才下载 dockview 分包。
const LazyChatDockView = lazy(() =>
  import('./ChatDockView').then((module) => ({ default: module.ChatDockView })),
)

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
  const localStreamSessionsRef = useRef(new Set<string>())
  const streamGenerationRef = useRef(new Map<string, number>())
  const resumeSyncRef = useRef<Promise<void> | null>(null)
  const catalog = useSessionCatalog({ notify })
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
    singleSessionLayout: mobileLayout,
    notify,
  })

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
  const { sendPrompt } = promptCommands

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
        // 先让原生壳复验内置 Runtime，避免 renderer/API 恢复时使用失效的代理上下文。
        await invokeMobile<void>('mobile_resume_local_runtime').catch(() => undefined)
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
        await refreshSessions(created.id)
        setRecallPulse((current) => ({ sessionId: session.id, token: current.token + 1 }))
        await new Promise<void>((resolve) => window.setTimeout(resolve, 550))
        openSessionInDock(created.id)
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
      sessionTreePulseSessionId: recallPulse.sessionId,
      sessionTreePulseToken: recallPulse.token,
      pendingAsset,
      onAssetConsumed,
      notify,
      openModelSettings,
      loadSessionMessages: liveSync.loadSessionMessages,
      loadOlderMessages: liveSync.loadOlderMessages,
      sendPrompt: promptCommands.sendPrompt,
      queuePrompt: promptCommands.queuePrompt,
      abort: promptCommands.abort,
      pauseGoal: sessionCommands.pauseGoal,
      setGoalBudget: sessionCommands.setGoalBudget,
      compactSession: sessionCommands.compactSession,
      setCompactionThreshold: sessionCommands.setCompactionThreshold,
      switchSessionModel: sessionCommands.switchSessionModel,
      loadSessionThinkingLevel: sessionCommands.loadSessionThinkingLevel,
      switchSessionThinkingLevel: sessionCommands.switchSessionThinkingLevel,
      switchSessionExecutionMode: sessionCommands.switchSessionExecutionMode,
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
      promptCommands.queuePrompt,
      promptCommands.abort,
      sessionCommands.pauseGoal,
      sessionCommands.setGoalBudget,
      sessionCommands.compactSession,
      sessionCommands.setCompactionThreshold,
      sessionCommands.switchSessionModel,
      sessionCommands.loadSessionThinkingLevel,
      sessionCommands.switchSessionThinkingLevel,
      sessionCommands.switchSessionExecutionMode,
      sessionCommands.resolveToolApproval,
      sessionCommands.selectSessionWorkspace,
      sessionCommands.renameSession,
      branchFromEntry,
      createChildSession,
      reloadSessionBranch,
    ],
  )

  return (
    <>
      <div className="chat-layout max-[900px]:grid-cols-[minmax(0,1fr)] max-[650px]:flex max-[650px]:flex-col max-[650px]:min-h-0 relative grid w-full min-w-0 min-h-0 flex-1 grid-cols-[minmax(0,1fr)] gap-[0] dock-layout">
        {catalog.loading ? (
          <AppEmptyState>
            <RefreshCw className="animate-spin" size={24} />
            <h2>{t('chat:chatPage.wakingTheAgent')}</h2>
            <p>{t('chat:chatPage.modelsSessionsAndContextAreSettlingIntoPlace')}</p>
          </AppEmptyState>
        ) : (
          <div className="chat-dock-workspace max-[650px]:[flex:1_1_0] max-[650px]:min-h-0 relative min-w-0 min-h-0 [isolation:isolate] overflow-hidden [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-md)] bg-[var(--panel)]">
            <ChatDockContext.Provider value={dockContextValue}>
              {clientLoaded && mobileLayout ? (
                <MobileSessionPanel
                  sessionIds={dock.mobileSessionIds}
                  onSelectSession={openSessionInDock}
                  onCreateSession={createSession}
                />
              ) : clientLoaded ? (
                <Suspense fallback={null}>
                  <LazyChatDockView
                    compactDock={dock.compactDock}
                    onDockReady={dock.onDockReady}
                    getTabContextMenuItems={dock.getTabContextMenuItems}
                    createSession={createSession}
                  />
                </Suspense>
              ) : null}
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
    </>
  )
}
