import { useContext, useEffect, useMemo, useState } from 'react'
import type { IDockviewPanelProps } from 'dockview-react'
import {
  AlertTriangle,
  MessageSquare,
  PanelLeft,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRight,
  Plus,
  Search,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { relativeTime } from '@/lib/format'
import { DEFAULT_SESSION_STATE } from '@/lib/session-state'
import type { ChatAttachment, SessionState, SessionSummary } from '@/types/chat'
import { FocusSession } from './FocusSession'
import { ChatDockContext } from './chat-dock-context'
import { sessionIdFromPanel, type SessionOpenDisposition } from './dock-layout'

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
        onApproval={(approvalId: string, approved: boolean) =>
          context.resolveToolApproval(sessionId, approvalId, approved)
        }
        onWorkspace={() => context.setWorkspaceSession(session)}
        onRename={() => context.renameSession(session)}
        onSplitLeft={() => context.splitDockPanel(api.id, 'left')}
        onSplitRight={() => context.splitDockPanel(api.id, 'right')}
        onClosePanel={() => context.closeDockPanel(api.id)}
        canSplit={!context.compactDock && api.group.size > 1}
        onSend={(value: string, attachments: ChatAttachment[], goalMode: boolean) =>
          context.sendPrompt(value, sessionId, attachments, goalMode)
        }
        onQueue={(value: string, behavior: string) =>
          context.queuePrompt(value, sessionId, behavior)
        }
        onAbort={() => context.abort(sessionId)}
        onOpenRail={context.openRail}
      />
    </div>
  )
}

export function ChatDockWatermark() {
  const context = useContext(ChatDockContext)
  const { t } = useI18n()
  return (
    <div className="chat-dock-watermark">
      <MessageSquare size={34} />
      <strong>{t('chat:chatDock.openAChatToBegin')}</strong>
      <span>{t('chat:chatDock.openAChatFromTheSessionListOrSplitItToTheLeftOrRight')}</span>
      {context?.openRail && (
        <button type="button" className="button secondary" onClick={context.openRail}>
          <PanelLeftOpen size={14} />
          {t('chat:chatDock.showChatList')}
        </button>
      )}
    </div>
  )
}

export function SessionRail({
  sessions,
  states,
  activeId,
  splitEnabled,
  onSelect,
  onSplit,
  onCreate,
  onClose,
}: {
  sessions: SessionSummary[]
  states: Record<string, SessionState>
  activeId: string
  splitEnabled: boolean
  onSelect: (sessionId: string) => void
  onSplit: (sessionId: string, direction: SessionOpenDisposition) => void
  onCreate: () => void
  onClose: () => void
}) {
  const { t, language } = useI18n()
  const [railQuery, setRailQuery] = useState('')
  const filtered = useMemo(() => {
    const keyword = railQuery.trim().toLowerCase()
    return [...sessions]
      .sort((a, b) => Date.parse(b.modified || '') - Date.parse(a.modified || ''))
      .filter(
        (session) =>
          !keyword ||
          `${session.name} ${session.firstMessage || ''}`.toLowerCase().includes(keyword),
      )
  }, [sessions, railQuery])
  return (
    <aside className="session-rail" aria-label={t('chat:chatDock.chatList')}>
      <div className="session-rail-head">
        <strong>{t('chat:chatDock.sessions')}</strong>
        <button
          className="icon-button"
          title={t('chat:chatDock.newChat')}
          aria-label={t('chat:chatDock.newChat')}
          onClick={onCreate}
        >
          <Plus size={15} />
        </button>
        <button
          className="icon-button"
          title={t('chat:chatDock.hideChatList')}
          aria-label={t('chat:chatDock.hideChatList')}
          onClick={onClose}
        >
          <PanelLeftClose size={15} />
        </button>
      </div>
      <label className="session-rail-search">
        <Search size={13} />
        <input
          value={railQuery}
          onChange={(event) => setRailQuery(event.target.value)}
          placeholder={t('chat:chatDock.searchChats')}
        />
      </label>
      <div className="session-rail-list">
        {filtered.map((session) => {
          const state = states[session.id]
          const streaming = Boolean(state?.streaming)
          const compacting = Boolean(state?.compaction?.active)
          return (
            <div
              className={`session-rail-row ${session.id === activeId ? 'active' : ''}`}
              key={session.id}
            >
              <button className="session-rail-item" onClick={() => onSelect(session.id)}>
                <span className="session-rail-item-name">
                  {streaming && <i className="session-rail-live" />}
                  {session.name || t('chat:chatDock.untitledChat')}
                </span>
                <span className="session-rail-item-meta">
                  {compacting
                    ? t('chat:chatDock.compactingContext')
                    : streaming
                      ? t('chat:chatDock.agentRunning')
                      : t('chat:chatDock.countMessages', { count: session.messageCount || 0 })}{' '}
                  · {relativeTime(session.modified, language)}
                </span>
              </button>
              {splitEnabled && (
                <div className="session-rail-split-actions">
                  <button
                    type="button"
                    title={t('chat:chatDock.splitToLeft')}
                    aria-label={t('chat:chatDock.splitNameToTheLeft', { name: session.name })}
                    onClick={() => onSplit(session.id, 'left')}
                  >
                    <PanelLeft size={13} />
                  </button>
                  <button
                    type="button"
                    title={t('chat:chatDock.splitToRight')}
                    aria-label={t('chat:chatDock.splitNameToTheRight', { name: session.name })}
                    onClick={() => onSplit(session.id, 'right')}
                  >
                    <PanelRight size={13} />
                  </button>
                </div>
              )}
            </div>
          )
        })}
        {!filtered.length && (
          <span className="session-rail-empty">
            {railQuery.trim()
              ? t('chat:chatDock.noMatchingChats')
              : t('chat:chatDock.noChatHistoryYet')}
          </span>
        )}
      </div>
    </aside>
  )
}
