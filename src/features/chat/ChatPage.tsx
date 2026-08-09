import { useCallback, useMemo, useRef } from 'react'
import 'dockview-react/dist/styles/dockview.css'
import {
  DockviewReact,
  type DockviewGroupPanel,
  type IDockviewHeaderActionsProps,
} from 'dockview-react'
import { Plus, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppCard as Panel } from '@/components/ui/app-primitives'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { Notify } from '@/app/route-context'
import type { PendingAsset } from '@/types/chat'
import { ChatDockWatermark, SessionDockPanel } from './ChatDock'
import { WebPreviewDockPanel } from './WebPreviewDockPanel'
import { ChatDockContext, type ChatDockContextValue } from './chat-dock-context'
import { useChatDock } from './use-chat-dock'
import { useLiveSessionSync } from './use-live-session-sync'
import { usePromptCommands } from './use-prompt-commands'
import { useSessionCatalog } from './use-session-catalog'
import { useSessionCommands } from './use-session-commands'

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
  const openSessionInDock = dock.openSessionInDock
  const moveSessionToGroup = dock.moveSessionToGroup
  const createSession = useCallback(
    async (targetGroup?: DockviewGroupPanel) => {
      const sessionId = await createSessionRecord()
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

  const DockNewSessionAction = useMemo(
    () =>
      function DockNewSessionAction({ group }: IDockviewHeaderActionsProps) {
        const label = t('navigation:pageHeader.newChat')
        return (
          <button
            type="button"
            className="dock-new-session"
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
    splitDockPanel: dock.splitDockPanel,
    closeDockPanel: dock.closeDockPanel,
  }

  return (
    <div className="chat-layout dock-layout">
      {catalog.loading ? (
        <Panel className="empty-state">
          <RefreshCw className="spin" size={24} />
          <h2>{t('chat:chatPage.wakingTheAgent')}</h2>
          <p>{t('chat:chatPage.modelsSessionsAndContextAreSettlingIntoPlace')}</p>
        </Panel>
      ) : (
        <div className="chat-dock-workspace">
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
  )
}
