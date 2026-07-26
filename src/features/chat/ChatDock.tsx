import { useContext, useEffect } from 'react'
import type { IDockviewPanelProps } from 'dockview-react'
import { AlertTriangle, MessageSquare } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { DEFAULT_SESSION_STATE } from '@/lib/session-state'
import type { ChatAttachment } from '@/types/chat'
import { FocusSession } from './FocusSession'
import { ChatDockContext } from './chat-dock-context'
import { sessionIdFromPanel } from './dock-layout'

const FOCUS_MESSAGE_PAGE_SIZE = 40

export function SessionDockPanel({ params, api }: IDockviewPanelProps<{ sessionId?: string }>) {
  const context = useContext(ChatDockContext)
  const sessionId = params?.sessionId || sessionIdFromPanel(api?.id)
  const session = context?.sessions.find((item) => item.id === sessionId)
  const state = context?.sessionStates[sessionId] || DEFAULT_SESSION_STATE
  const loadMessages = context?.loadSessionMessages

  useEffect(() => {
    if (sessionId) void loadMessages?.(sessionId, { limit: FOCUS_MESSAGE_PAGE_SIZE })
  }, [loadMessages, sessionId])

  if (!context || !session) {
    return (
      <div className="session-dock-missing">
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
    <div className="session-dock-panel" onFocusCapture={() => api.setActive()}>
      <FocusSession
        session={session}
        messages={state.messages || []}
        messageStart={state.messageStart}
        hasOlder={state.hasOlder}
        loadingOlder={state.loadingOlder}
        olderError={state.olderError}
        model={state.model || session.model || context.defaultModel}
        executionMode={state.executionMode || session.executionMode || 'workspace'}
        sandboxStatus={context.sandboxStatus}
        goal={state.goal ?? session.goal ?? null}
        currentActivity={state.currentActivity}
        activityFeed={state.activityFeed || []}
        tools={state.tools || []}
        thinkingText={state.thinkingText || ''}
        queuedInputs={state.queuedInputs || []}
        compaction={state.compaction}
        contextUsage={state.contextUsage}
        cwd={state.cwd || session.cwd}
        availableModels={context.availableModels}
        switchingModel={state.switchingModel}
        switchingCwd={state.switchingCwd}
        switchingPermission={state.switchingPermission}
        streaming={state.streaming}
        runStartedAt={state.runStartedAt}
        lastActivityAt={state.lastActivityAt}
        runFinishedAt={state.runFinishedAt}
        runStopped={state.runStopped}
        runNotice={state.runNotice}
        approvals={state.approvals || []}
        error={state.error || (context.activeId === sessionId ? context.globalError : '')}
        pendingAsset={pending}
        onAssetConsumed={context.onAssetConsumed}
        onLoadOlder={() => context.loadOlderMessages(sessionId)}
        onModelChange={(nextModel: string) => context.switchSessionModel(sessionId, nextModel)}
        onExecutionModeChange={(nextMode: string) =>
          context.switchSessionExecutionMode(sessionId, nextMode)
        }
        onGoalPause={() => context.pauseGoal(sessionId)}
        onGoalBudgetChange={(tokenBudget: number) => context.setGoalBudget(sessionId, tokenBudget)}
        onApproval={(approvalId: string, approved: boolean) =>
          context.resolveToolApproval(sessionId, approvalId, approved)
        }
        onWorkspace={() => context.setWorkspaceSession(session)}
        onRename={() => context.renameSession(session)}
        onSplitLeft={() => context.splitDockPanel(api.id, 'left')}
        onSplitRight={() => context.splitDockPanel(api.id, 'right')}
        onSplitTop={() => context.splitDockPanel(api.id, 'above')}
        onSplitBottom={() => context.splitDockPanel(api.id, 'below')}
        onClosePanel={() => context.closeDockPanel(api.id)}
        canSplit={!context.compactDock && api.group.size > 1}
        onSend={(
          value: string,
          attachments: ChatAttachment[],
          goalMode: boolean,
          goalTokenBudget: number | null,
        ) => context.sendPrompt(value, sessionId, attachments, goalMode, goalTokenBudget)}
        onQueue={(value: string, behavior: string) =>
          context.queuePrompt(value, sessionId, behavior)
        }
        onAbort={() => context.abort(sessionId)}
      />
    </div>
  )
}

export function ChatDockWatermark() {
  const { t } = useI18n()
  return (
    <div className="chat-dock-watermark">
      <MessageSquare size={34} />
      <strong>{t('chat:chatDock.openAChatToBegin')}</strong>
      <span>{t('chat:chatDock.openAChatFromTheSessionListOrSplitItInAnyDirection')}</span>
    </div>
  )
}
