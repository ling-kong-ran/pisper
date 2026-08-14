import { useEffect, useRef, useState, type FormEvent } from 'react'
import {
  Braces,
  ChevronsLeft,
  ChevronsRight,
  Command,
  File,
  FolderOpen,
  Minimize2,
  Paperclip,
  RefreshCw,
  Send,
  Square,
  Wrench,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { QueueSection } from '@/components/ai-elements/queue'
import { AppCard as Panel } from '@/components/ui/app-primitives'
import { formatFileSize, workspaceName } from '@/lib/format'
import type {
  ChatAttachment,
  ChatMessage,
  EntityRecord,
  ModelOption,
  Plan,
  ResourceInvocation,
  SessionSummary,
} from '@/types/chat'
import { pathAttachments, useAttachmentSelection } from './attachments'
import { ChatResourcePicker } from './ChatResourcePicker'
import { ComposerToolTray } from './ComposerToolTray'
import { requestCommandPalette } from './events'
import {
  ContextUsageIndicator,
  ExecutionModeSelect,
  SessionModelSelect,
  SessionThinkingSelect,
  SessionUsageMetrics,
} from './FocusRuntimeControls'
import { FocusTranscript } from './FocusTranscript'
import { GitChangesControl } from './GitChangesControl'
import { GoalModeControl } from './GoalModeControl'
import { PathAttachmentPicker } from './PathAttachmentPicker'
import { SessionActionsMenu } from './SessionActionsMenu'
import { SessionWorkflowRuns } from './SessionWorkflowRuns'
import { ToolApproval } from './ToolApproval'
import { WorkspaceTrustNotice } from './WorkspaceTrustNotice'

const DEFAULT_GOAL_TOKEN_BUDGET = 30_000
const COMMAND_PALETTE_SHORTCUT =
  typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform)
    ? '\u2318 K'
    : 'Ctrl K'

export type FocusSessionProps = {
  session: SessionSummary
  messages: ChatMessage[]
  messageStart?: number | null
  hasOlder?: boolean
  loadingOlder?: boolean
  olderError?: string
  model: string
  thinkingLevel?: string
  availableThinkingLevels?: string[]
  thinkingStatus?: string
  thinkingMessage?: string
  executionMode: string
  goal?: EntityRecord | null
  plan?: Plan | null
  currentActivity?: EntityRecord | null
  activityFeed: EntityRecord[]
  tools: EntityRecord[]
  thinkingText?: string
  queuedInputs: EntityRecord[]
  compaction?: EntityRecord | null
  contextUsage?: EntityRecord | null
  sessionUsage?: EntityRecord | null
  cwd?: string
  availableModels: ModelOption[]
  switchingModel?: boolean
  switchingThinking?: boolean
  switchingCwd?: boolean
  switchingPermission?: boolean
  streaming?: boolean
  runStartedAt?: string | null
  lastActivityAt?: string | null
  runFinishedAt?: string | null
  runStopped?: boolean
  runNotice?: string
  approvals: EntityRecord[]
  error?: string
  pendingAsset?: ChatAttachment | null
  canSplit?: boolean
  onAssetConsumed?: () => void
  onLoadOlder?: () => Promise<boolean> | boolean
  onModelChange: (model: string) => Promise<void> | void
  onThinkingLevelChange: (level: string) => Promise<void> | void
  onExecutionModeChange: (mode: string) => Promise<boolean> | boolean
  onGoalPause?: () => Promise<void> | void
  onGoalBudgetChange?: (tokenBudget: number) => Promise<void> | void
  onCompact?: () => Promise<void> | void
  onCompactionThresholdChange?: (thresholdPercent: number) => Promise<void> | void
  onApproval: (approvalId: string, approved: boolean) => Promise<void> | void
  onWorkspace: () => void
  onRename: () => void
  onDerive: (boundaryEntryId: string) => Promise<void> | void
  onSplitLeft: () => void
  onSplitRight: () => void
  onSplitTop: () => void
  onSplitBottom: () => void
  onClosePanel: () => void
  onSend: (
    value: string,
    attachments: ChatAttachment[],
    goalMode: boolean,
    goalTokenBudget: number | null,
    invocation?: ResourceInvocation | null,
  ) => Promise<void> | void
  onQueue?: (value: string, behavior: string) => Promise<boolean> | boolean
  onAbort: () => Promise<void> | void
}

export function FocusSession({
  session,
  messages,
  messageStart,
  hasOlder,
  loadingOlder,
  olderError,
  model,
  thinkingLevel,
  availableThinkingLevels,
  thinkingStatus,
  thinkingMessage,
  executionMode,
  goal,
  plan,
  currentActivity,
  activityFeed,
  tools,
  thinkingText,
  queuedInputs,
  compaction,
  contextUsage,
  sessionUsage,
  cwd,
  availableModels,
  switchingModel,
  switchingThinking,
  switchingCwd,
  switchingPermission,
  streaming,
  runStartedAt,
  lastActivityAt,
  runFinishedAt,
  runStopped,
  runNotice,
  approvals,
  error,
  pendingAsset,
  canSplit,
  onAssetConsumed,
  onLoadOlder,
  onModelChange,
  onThinkingLevelChange,
  onExecutionModeChange,
  onCompact,
  onCompactionThresholdChange,
  onGoalPause,
  onGoalBudgetChange,
  onApproval,
  onWorkspace,
  onRename,
  onDerive,
  onSplitLeft,
  onSplitRight,
  onSplitTop,
  onSplitBottom,
  onClosePanel,
  onSend,
  onQueue,
  onAbort,
}: FocusSessionProps) {
  const { t, language } = useI18n()
  const [value, setValue] = useState('')
  const [goalArmed, setGoalArmed] = useState(false)
  const [goalTokenBudget, setGoalTokenBudget] = useState(DEFAULT_GOAL_TOKEN_BUDGET)
  const [queueing, setQueueing] = useState(false)
  const [compactingManually, setCompactingManually] = useState(false)
  const [scrollRequest, setScrollRequest] = useState(0)
  const [resourcePickerOpen, setResourcePickerOpen] = useState(false)
  const [pathPickerOpen, setPathPickerOpen] = useState(false)
  const [toolsOpen, setToolsOpen] = useState(false)
  const [invocation, setInvocation] = useState<ResourceInvocation | null>(null)
  const selection = useAttachmentSelection()
  const addSelectedAttachments = selection.addAttachments
  const promptRef = useRef<HTMLTextAreaElement>(null)
  const hasConversation = messages.length > 0
  const toolTrayId = `composer-tool-tray-${session.id}`
  const quickActionsLabel = toolsOpen
    ? t('chat:focusSession.collapseQuickActions')
    : t('chat:focusSession.expandQuickActions')

  useEffect(() => {
    setGoalArmed(false)
    setQueueing(false)
    setCompactingManually(false)
    setInvocation(null)
    setResourcePickerOpen(false)
    setToolsOpen(false)
  }, [session.id])

  useEffect(() => {
    if (!pendingAsset) return
    addSelectedAttachments([pendingAsset])
    onAssetConsumed?.()
  }, [pendingAsset, onAssetConsumed, addSelectedAttachments])

  useEffect(() => {
    if (!toolsOpen) return undefined
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setToolsOpen(false)
    }
    document.addEventListener('keydown', close)
    return () => document.removeEventListener('keydown', close)
  }, [toolsOpen])

  const applyWelcomeChip = (prompt: string) => {
    setValue(prompt)
    requestAnimationFrame(() => {
      const element = promptRef.current
      if (!element) return
      element.focus()
      element.style.height = 'auto'
      element.style.height = `${Math.min(element.scrollHeight, 220)}px`
    })
  }

  const requestTranscriptBottom = () => {
    setScrollRequest((current) => current + 1)
  }

  const choosePathAttachments = async () => {
    if (window.pisperDesktop?.pickFiles) {
      try {
        const paths = await window.pisperDesktop.pickFiles(cwd)
        selection.addAttachments(pathAttachments(paths || []))
      } catch (caught) {
        selection.setError(caught)
      }
      return
    }
    setPathPickerOpen(true)
  }

  const compactContext = async () => {
    if (!onCompact || streaming || compactingManually || compaction?.active) return
    setCompactingManually(true)
    try {
      await onCompact()
    } finally {
      setCompactingManually(false)
    }
  }

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!value.trim() && !selection.attachments.length && !invocation) return
    if (streaming) {
      if (!value.trim() || queueing) return
      setQueueing(true)
      const queued = await onQueue?.(value, 'steer')
      setQueueing(false)
      if (!queued) return
      setValue('')
      setGoalArmed(false)
      requestTranscriptBottom()
      if (promptRef.current) promptRef.current.style.height = 'auto'
      return
    }
    onSend(value, selection.attachments, goalArmed, goalArmed ? goalTokenBudget : null, invocation)
    requestTranscriptBottom()
    setValue('')
    setGoalArmed(false)
    setInvocation(null)
    if (promptRef.current) promptRef.current.style.height = 'auto'
    selection.clearAttachments()
  }

  return (
    <Panel className={`focus-session ${hasConversation ? 'has-conversation' : 'is-empty'}`}>
      {!hasConversation && (
        <div className="card-head">
          <div className="focus-session-head-actions">
            <SessionActionsMenu
              session={session}
              canSplit={canSplit}
              streaming={streaming}
              switchingCwd={switchingCwd}
              onSplitLeft={onSplitLeft}
              onSplitRight={onSplitRight}
              onSplitTop={onSplitTop}
              onSplitBottom={onSplitBottom}
              onClosePanel={onClosePanel}
              onWorkspace={onWorkspace}
              onRename={onRename}
            />
          </div>
        </div>
      )}

      <FocusTranscript
        sessionId={session.id}
        messages={messages}
        messageStart={messageStart}
        hasOlder={hasOlder}
        loadingOlder={loadingOlder}
        olderError={olderError}
        currentActivity={currentActivity}
        activityFeed={activityFeed}
        tools={tools}
        thinkingText={thinkingText}
        compaction={compaction}
        streaming={streaming}
        runStartedAt={runStartedAt}
        lastActivityAt={lastActivityAt}
        runFinishedAt={runFinishedAt}
        runStopped={runStopped}
        runNotice={runNotice}
        error={error}
        scrollRequest={scrollRequest}
        cwd={cwd}
        lineage={session.lineage}
        switchingCwd={switchingCwd}
        onLoadOlder={onLoadOlder}
        onDerive={onDerive}
        onPromptSelect={applyWelcomeChip}
        onWorkspace={onWorkspace}
      />

      <form className="focus-composer-shell" onSubmit={submit}>
        <WorkspaceTrustNotice sessionId={session.id} cwd={cwd} streaming={streaming} />
        <ToolApproval approvals={approvals} onResolve={onApproval} />
        {streaming && queuedInputs.length > 0 && (
          <QueueSection asChild defaultOpen>
            <div className="queued-input-tray" data-pisper-queue-size={queuedInputs.length}>
              <span>{t('chat:focusSession.sentToTheRunningAgent')}</span>
              {queuedInputs.slice(-3).map((item, index) => (
                <small key={item.id || `${item.behavior}-${index}`} title={item.text}>
                  {item.text}
                </small>
              ))}
              {queuedInputs.length > 3 && (
                <em>{t('chat:focusSession.countMore', { count: queuedInputs.length - 3 })}</em>
              )}
            </div>
          </QueueSection>
        )}
        <SessionWorkflowRuns sessionId={session.id} />
        {invocation && (
          <div className={`composer-resource-chip ${invocation.kind}`}>
            {invocation.kind === 'tool' ? <Wrench size={13} /> : <Braces size={13} />}
            <span>
              {invocation.kind === 'skill'
                ? 'Skill'
                : invocation.kind === 'tool'
                  ? t('chat:resourcePicker.tool')
                  : t('chat:resourcePicker.workflow')}{' '}
              · {invocation.resourceName}
            </span>
            <button
              type="button"
              aria-label={t('chat:resourcePicker.remove')}
              onClick={() => setInvocation(null)}
            >
              <X size={13} />
            </button>
          </div>
        )}
        <AttachmentTray attachments={selection.attachments} onRemove={selection.removeAttachment} />
        {selection.attachmentError && (
          <span className="attachment-error">{selection.attachmentError}</span>
        )}
        <div
          className={`focus-composer-status ${compaction?.active ? 'compacting' : streaming ? 'running' : 'idle'}`}
          role="status"
          aria-live="polite"
        >
          <i aria-hidden="true" />
          <span>
            {compaction?.active
              ? t('chat:focusSession.compactingContext')
              : streaming
                ? t('chat:focusSession.running')
                : t('chat:focusSession.waitingForInput')}
          </span>
        </div>
        <div className="focus-composer">
          <textarea
            ref={promptRef}
            rows={1}
            value={value}
            onChange={(event) => {
              setValue(event.target.value)
              event.currentTarget.style.height = 'auto'
              event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 220)}px`
            }}
            onPaste={streaming ? undefined : selection.pasteImages}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.shiftKey) {
                event.preventDefault()
                event.currentTarget.form?.requestSubmit()
              }
            }}
            placeholder={
              streaming
                ? t('chat:focusSession.addGuidanceForTheRunningAgentShiftEnterForANewLine')
                : t('chat:focusSession.writeWhatYouWantToAccomplishShiftEnterForANewLine')
            }
          />
          <div className={`focus-composer-footer ${toolsOpen ? 'tools-open' : ''}`}>
            <div className="focus-composer-leading">
              <button
                type="button"
                className="resource-picker-trigger"
                title={t('chat:resourcePicker.open')}
                aria-label={t('chat:resourcePicker.open')}
                onClick={() => setResourcePickerOpen(true)}
              >
                <Braces size={16} />
              </button>
              <button
                type="button"
                className="attach-trigger"
                title={t('chat:focusSession.addAttachment')}
                aria-label={t('chat:focusSession.addAttachment')}
                onClick={() => void choosePathAttachments()}
                disabled={streaming}
              >
                <Paperclip size={17} />
                {selection.attachments.length > 0 && <i>{selection.attachments.length}</i>}
              </button>
            </div>
            <div className="focus-composer-runtime">
              <SessionModelSelect
                value={model}
                models={availableModels}
                onChange={onModelChange}
                disabled={streaming || switchingModel}
              />
              <ExecutionModeSelect
                value={executionMode}
                onChange={onExecutionModeChange}
                disabled={switchingPermission}
              />
            </div>
            <div className="focus-composer-quick-actions">
              <button
                type="button"
                className={`composer-tools-trigger ${toolsOpen ? 'active' : ''}`}
                title={quickActionsLabel}
                aria-label={quickActionsLabel}
                aria-expanded={toolsOpen}
                aria-controls={toolTrayId}
                onClick={() => setToolsOpen((open) => !open)}
              >
                {toolsOpen ? <ChevronsLeft size={17} /> : <ChevronsRight size={17} />}
              </button>
              <ComposerToolTray
                open={toolsOpen}
                label={t('chat:focusSession.quickActions')}
                trayId={toolTrayId}
              >
                <button
                  type="button"
                  className="command-palette-trigger"
                  title={t('chat:focusSession.openCommandPaletteShortcut', {
                    shortcut: COMMAND_PALETTE_SHORTCUT,
                  })}
                  aria-label={t('chat:focusSession.openCommandPaletteShortcut', {
                    shortcut: COMMAND_PALETTE_SHORTCUT,
                  })}
                  onClick={requestCommandPalette}
                >
                  <Command size={16} />
                  <kbd>{COMMAND_PALETTE_SHORTCUT}</kbd>
                </button>
                <SessionThinkingSelect
                  value={thinkingLevel || 'medium'}
                  levels={availableThinkingLevels || []}
                  status={thinkingStatus}
                  message={thinkingMessage}
                  onChange={onThinkingLevelChange}
                  disabled={streaming || switchingThinking || switchingModel}
                />
                <GoalModeControl
                  goal={goal}
                  armed={goalArmed}
                  tokenBudget={goalTokenBudget}
                  onTokenBudgetChange={setGoalTokenBudget}
                  onSaveTokenBudget={(tokenBudget) => onGoalBudgetChange?.(tokenBudget)}
                  onChange={(enabled) => {
                    if (!enabled && goal?.status === 'active') void onGoalPause?.()
                    else setGoalArmed(enabled)
                  }}
                />
                <GitChangesControl sessionId={session.id} streaming={streaming} />
                <button
                  type="button"
                  className="compact-context-trigger"
                  title={
                    streaming
                      ? t('chat:focusSession.manualCompactionWaitForRun')
                      : compactingManually || compaction?.active
                        ? t('chat:focusSession.compactingContext')
                        : t('chat:focusSession.compactContextNow')
                  }
                  aria-label={t('chat:focusSession.compactContextNow')}
                  disabled={
                    !onCompact ||
                    streaming ||
                    compactingManually ||
                    Boolean(compaction?.active) ||
                    messages.length === 0
                  }
                  onClick={() => void compactContext()}
                >
                  {compactingManually || compaction?.active ? (
                    <RefreshCw className="spin" size={14} />
                  ) : (
                    <Minimize2 size={14} />
                  )}
                </button>
                {hasConversation && (
                  <div className="focus-composer-session-actions">
                    <SessionActionsMenu
                      session={session}
                      canSplit={canSplit}
                      streaming={streaming}
                      switchingCwd={switchingCwd}
                      onSplitLeft={onSplitLeft}
                      onSplitRight={onSplitRight}
                      onSplitTop={onSplitTop}
                      onSplitBottom={onSplitBottom}
                      onClosePanel={onClosePanel}
                      onWorkspace={onWorkspace}
                      onRename={onRename}
                    />
                  </div>
                )}
              </ComposerToolTray>
            </div>
            <div className="focus-composer-secondary">
              <ContextUsageIndicator
                usage={contextUsage}
                onThresholdChange={onCompactionThresholdChange}
              />
            </div>
            <button
              type={streaming ? 'button' : 'submit'}
              className={`send-button${streaming ? ' stop' : ''}`}
              title={streaming ? t('chat:focusSession.stop') : t('chat:focusSession.sendMessage')}
              aria-label={
                streaming ? t('chat:focusSession.stop') : t('chat:focusSession.sendMessage')
              }
              onClick={streaming ? onAbort : undefined}
              disabled={
                !streaming &&
                (queueing || (!value.trim() && !selection.attachments.length && !invocation))
              }
            >
              {streaming ? (
                <Square size={16} fill="currentColor" />
              ) : queueing ? (
                <RefreshCw className="spin" size={17} />
              ) : (
                <Send size={18} />
              )}
            </button>
          </div>
        </div>
        <div className="focus-composer-meta">
          {hasConversation && (
            <button
              type="button"
              className="composer-workspace-status"
              title={cwd}
              aria-label={t('chat:focusSession.changeWorkingDirectoryWorkspace', {
                workspace: workspaceName(cwd, language),
              })}
              onClick={onWorkspace}
              disabled={streaming || switchingCwd}
            >
              <FolderOpen size={12} />
              <span>{workspaceName(cwd, language)}</span>
            </button>
          )}
          <SessionUsageMetrics usage={sessionUsage} plan={plan} />
        </div>
      </form>
      <ChatResourcePicker
        open={resourcePickerOpen}
        sessionId={session.id}
        onClose={() => setResourcePickerOpen(false)}
        onSelect={setInvocation}
      />
      <PathAttachmentPicker
        open={pathPickerOpen}
        initialPath={cwd}
        onOpenChange={setPathPickerOpen}
        onSelect={(paths) => selection.addAttachments(pathAttachments(paths))}
      />
    </Panel>
  )
}

function AttachmentTray({
  attachments,
  onRemove,
  compact = false,
}: {
  attachments: ChatAttachment[]
  onRemove: (id: string) => void
  compact?: boolean
}) {
  const { t } = useI18n()
  if (!attachments.length) return null
  return (
    <div className={`attachment-tray ${compact ? 'compact' : ''}`}>
      {attachments.map((attachment) => (
        <div className="attachment-chip" key={attachment.id}>
          {attachment.kind === 'image' ? (
            <img src={`data:${attachment.mimeType};base64,${attachment.data}`} alt="" />
          ) : (
            <span className="attachment-icon">
              <File size={13} />
            </span>
          )}
          <span>
            <strong>{attachment.name}</strong>
            <small>
              {attachment.kind === 'path'
                ? t('chat:focusSession.localPath')
                : attachment.kind === 'image'
                  ? t('chat:focusSession.image')
                  : attachment.kind === 'document'
                    ? t('chat:focusSession.document')
                    : t('chat:focusSession.text')}
              {attachment.kind !== 'path' ? ` · ${formatFileSize(attachment.size)}` : ''}
              {attachment.truncated ? ` · ${t('chat:focusSession.truncated')}` : ''}
            </small>
          </span>
          <button
            type="button"
            aria-label={t('chat:focusSession.removeName', { name: attachment.name })}
            onClick={() => onRemove(String(attachment.id || ''))}
          >
            <X size={12} />
          </button>
        </div>
      ))}
    </div>
  )
}
