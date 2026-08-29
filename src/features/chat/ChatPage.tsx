// 聊天主页面：dockview 多会话分屏布局的宿主，持有会话目录与实时
// 同步状态，管理 Dock 的初始化/持久化与多面板交互。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import 'dockview-react/dist/styles/dockview.css'
import {
  DockviewReact,
  type DockviewGroupPanel,
  type IDockviewHeaderActionsProps,
} from 'dockview-react'
import { Plus, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { AppEmptyState } from '@/components/ui/app-primitives'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { Notify } from '@/app/route-context'
import type { PendingAsset, SessionSummary } from '@/types/chat'
import { ChatDockWatermark, SessionDockPanel } from './ChatDock'
import { chatApi } from './chat-api'
import { WebPreviewDockPanel } from './WebPreviewDockPanel'
import { ChatDockContext, type ChatDockContextValue } from './chat-dock-context'
import { useChatDock } from './use-chat-dock'
import { useLiveSessionSync } from './use-live-session-sync'
import { usePromptCommands } from './use-prompt-commands'
import { useSessionCatalog } from './use-session-catalog'
import { useSessionCommands } from './use-session-commands'
import { SESSION_CREATE_REQUESTED_EVENT, consumeSessionCreationRequest } from './events'

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
      const sessionId = await createSessionRecord(cwd)
      if (!sessionId) return ''
      const opened = openSessionInDock(sessionId)
      if (opened) moveSessionToGroup(sessionId, targetGroup)
      return sessionId
    },
    [createSessionRecord, moveSessionToGroup, openSessionInDock],
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
      let sessions: SessionSummary[] = []
      try {
        sessions = await refreshSessions()
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
  }, [catalog.activeId, refreshSessions, sessionStatesRef, syncLiveSession])

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

  const DockNewSessionAction = useMemo(
    () =>
      function DockNewSessionAction({ group }: IDockviewHeaderActionsProps) {
        const label = t('navigation:pageHeader.newChat')
        return (
          <button
            type="button"
            className="dock-new-session hover:bg-[var(--surface-hover)] hover:text-[var(--text)] focus-visible:relative focus-visible:z-[1] focus-visible:[outline:2px_solid_var(--focus)] focus-visible:[outline-offset:-2px] grid w-[36px] h-[35px] flex-none place-items-center border-0 [border-left:1px_solid_var(--stroke-soft)] bg-transparent text-[var(--text-muted)] cursor-pointer"
            title={label}
            aria-label={label}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
              event.stopPropagation()
              void createSession(group)
            }}
          >
            <Plus size={15} />
          </button>
        )
      },
    [createSession, t],
  )
  const dockComponents = useMemo(
    () => ({ session: SessionDockPanel, webPreview: WebPreviewDockPanel }),
    [],
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
              <DockviewReact
                className="dockview-theme-light dockview-theme-pisper"
                components={dockComponents}
                watermarkComponent={ChatDockWatermark}
                leftHeaderActionsComponent={DockNewSessionAction}
                onReady={dock.onDockReady}
                getTabContextMenuItems={dock.getTabContextMenuItems}
                disableFloatingGroups
                disableDnd={dock.compactDock}
                noPanelsOverlay="watermark"
              />
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
