// 聊天 Dock 面板容器：每个 dockview 面板对应一个会话视图，
// 负责把面板事件（关闭/激活）桥接到会话状态与布局持久化。
import { useContext, useEffect, useRef, useState } from 'react'
import type { IDockviewPanelProps } from 'dockview-react'
import { AlertTriangle, MessageSquare } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { DEFAULT_SESSION_STATE, isPlanActive, resolveSessionPlan } from '@/lib/session-state'
import type { ChatAttachment } from '@/types/chat'
import { FocusSession } from './FocusSession'
import { ChatDockContext } from './chat-dock-context'
import { sessionIdFromPanel } from './dock-layout'

const FOCUS_MESSAGE_PAGE_SIZE = 40

export function SessionDockPanel({ params, api }: IDockviewPanelProps<{ sessionId?: string }>) {
  const context = useContext(ChatDockContext)
  const sessionId = params?.sessionId || sessionIdFromPanel(api?.id)
  const [visible, setVisible] = useState(() => api.isVisible)
  const session = context?.sessions.find((item) => item.id === sessionId)
  const sessionState = context?.sessionStates[sessionId]
  const state = sessionState || DEFAULT_SESSION_STATE
  const streaming = Boolean(state.streaming || session?.streaming)
  const plan = resolveSessionPlan(sessionState, session)
  const visiblePlan = isPlanActive(plan, { streaming }) ? plan : null
  const loadMessages = context?.loadSessionMessages
  const loadThinkingLevel = context?.loadSessionThinkingLevel
  const thinkingRequestedRef = useRef('')

  useEffect(() => {
    setVisible(api.isVisible)
    const disposable = api.onDidVisibilityChange(({ isVisible }) => setVisible(isVisible))
    return () => disposable.dispose()
  }, [api])

  useEffect(() => {
    if (visible && sessionId) void loadMessages?.(sessionId, { limit: FOCUS_MESSAGE_PAGE_SIZE })
  }, [loadMessages, sessionId, visible])

  // Self-heal thinking state for sessions loaded while streaming or after page remounts.
  useEffect(() => {
    if (!visible || !sessionId || !session || !loadThinkingLevel) return
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
    visible,
  ])

  if (!visible) return null

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
      onFocusCapture={() => api.setActive()}
    >
      <FocusSession
        session={session}
        messages={state.messages || []}
        transcriptLoadState={
          state.loaded ? 'ready' : state.loading || !state.error ? 'loading' : 'error'
        }
        messageStart={state.messageStart}
        hasOlder={state.hasOlder}
        loadingOlder={state.loadingOlder}
        olderError={state.olderError}
        model={state.model || session.model || context.defaultModel}
        thinkingLevel={state.thinkingLevel || session.thinkingLevel || 'medium'}
        availableThinkingLevels={state.availableThinkingLevels || []}
        thinkingStatus={state.thinkingStatus || ''}
        thinkingMessage={state.thinkingMessage || ''}
        executionMode={state.executionMode || session.executionMode || 'approval-required'}
        goal={state.goal ?? session.goal ?? null}
        plan={visiblePlan}
        currentActivity={state.currentActivity}
        activityFeed={state.activityFeed || []}
        tools={state.tools || []}
        thinkingText={state.thinkingText || ''}
        queuedInputs={state.queuedInputs || []}
        compaction={state.compaction}
        contextUsage={state.contextUsage}
        sessionUsage={state.sessionUsage}
        sessionTreeRevision={state.sessionTreeRevision}
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
        approvals={state.approvals || []}
        error={state.error || (context.activeId === sessionId ? context.globalError : '')}
        pendingAsset={pending}
        onAssetConsumed={context.onAssetConsumed}
        onLoadOlder={() => context.loadOlderMessages(sessionId)}
        onModelChange={(nextModel: string) => context.switchSessionModel(sessionId, nextModel)}
        onThinkingLevelChange={(nextLevel: string) =>
          context.switchSessionThinkingLevel(sessionId, nextLevel)
        }
        onExecutionModeChange={(nextMode: string) =>
          context.switchSessionExecutionMode(sessionId, nextMode)
        }
        onGoalPause={() => context.pauseGoal(sessionId)}
        onGoalBudgetChange={(tokenBudget: number) => context.setGoalBudget(sessionId, tokenBudget)}
        onCompact={() => context.compactSession(sessionId)}
        onCompactionThresholdChange={context.setCompactionThreshold}
        onApproval={(approvalId: string, approved: boolean) =>
          context.resolveToolApproval(sessionId, approvalId, approved)
        }
        onWorkspace={() => void context.selectSessionWorkspace(session)}
        onRename={() => context.renameSession(session)}
        onBranchFromHere={(boundaryEntryId: string) =>
          context.branchFromEntry(session, boundaryEntryId)
        }
        onCreateChildSession={(boundaryEntryId: string) =>
          context.createChildSession(session, boundaryEntryId)
        }
        onTreeNavigated={() => context.reloadSessionBranch(sessionId)}
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
          invocation,
        ) =>
          context.sendPrompt(value, sessionId, attachments, goalMode, goalTokenBudget, invocation)
        }
        onQueue={(value: string, attachments: ChatAttachment[], behavior: string) =>
          context.queuePrompt(value, sessionId, attachments, behavior)
        }
        onAbort={() => context.abort(sessionId)}
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
