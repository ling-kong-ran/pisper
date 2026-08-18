// 聊天主页面：dockview 多会话分屏布局的宿主，持有会话目录与实时
// 同步状态，管理 Dock 的初始化/持久化与多面板交互。
import { useCallback, useEffect, useMemo, useRef } from 'react'
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
  browserNotify?: (event: string, data: unknown, options?: { force?: boolean }) => void
  registerPrimaryAction: (action: () => void) => () => void
  pendingAsset: PendingAsset | null
  onAssetConsumed: () => void
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

export function ChatPage({
  notify,
  browserNotify,
  registerPrimaryAction,
  pendingAsset,
  onAssetConsumed,
  requestText,
  requestConfirm,
}: ChatPageProps) {
  const { t } = useI18n()
  const localStreamSessionsRef = useRef(new Set<string>())
  const catalog = useSessionCatalog({ notify })
  const liveSync = useLiveSessionSync({
    sessionStates: catalog.sessionStates,
    sessionStatesRef: catalog.sessionStatesRef,
    localStreamSessionsRef,
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
  const setGlobalError = catalog.setGlobalError
  const openSessionInDock = dock.openSessionInDock
  const moveSessionToGroup = dock.moveSessionToGroup
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

  useEffect(() => {
    const createRequested = () => {
      const request = consumeSessionCreationRequest()
      if (request) void createSession(undefined, request.cwd)
    }
    window.addEventListener(SESSION_CREATE_REQUESTED_EVENT, createRequested)
    createRequested()
    return () => window.removeEventListener(SESSION_CREATE_REQUESTED_EVENT, createRequested)
  }, [createSession])

  const promptCommands = usePromptCommands({
    browserNotify,
    notify,
    defaultModel: catalog.defaultModel,
    localStreamSessionsRef,
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

  const dockContextValue: ChatDockContextValue = {
    sessions: catalog.sessions,
    sessionStates: catalog.sessionStates,
    defaultModel: catalog.defaultModel,
    availableModels: catalog.availableModels,
    globalError: catalog.globalError,
    activeId: catalog.activeId,
    compactDock: dock.compactDock,
    pendingAsset,
    onAssetConsumed,
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
  }

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
