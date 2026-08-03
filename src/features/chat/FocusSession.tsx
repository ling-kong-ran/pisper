import { useEffect, useRef, useState, type FormEvent } from 'react'
import { Command, File, FolderOpen, Paperclip, RefreshCw, Send, Square, X } from 'lucide-react'
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
  SessionSummary,
} from '@/types/chat'
import { useAttachmentSelection } from './attachments'
import { requestCommandPalette } from './events'
import {
  ContextUsageIndicator,
  ExecutionModeSelect,
  SessionModelSelect,
} from './FocusRuntimeControls'
import { FocusTranscript } from './FocusTranscript'
import { GitChangesControl } from './GitChangesControl'
import { GoalModeControl } from './GoalModeControl'
import { SessionActionsMenu } from './SessionActionsMenu'
import { ToolApproval } from './ToolApproval'

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
  cwd?: string
  availableModels: ModelOption[]
  switchingModel?: boolean
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
  onExecutionModeChange: (mode: string) => Promise<boolean> | boolean
  onGoalPause?: () => Promise<void> | void
  onGoalBudgetChange?: (tokenBudget: number) => Promise<void> | void
  onCompactionThresholdChange?: (thresholdPercent: number) => Promise<void> | void
  onApproval: (approvalId: string, approved: boolean) => Promise<void> | void
  onWorkspace: () => void
  onRename: () => void
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
  cwd,
  availableModels,
  switchingModel,
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
  onExecutionModeChange,
  onCompactionThresholdChange,
  onGoalPause,
  onGoalBudgetChange,
  onApproval,
  onWorkspace,
  onRename,
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
  const [scrollRequest, setScrollRequest] = useState(0)
  const selection = useAttachmentSelection()
  const addSelectedAttachments = selection.addAttachments
  const promptRef = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    setGoalArmed(false)
    setQueueing(false)
  }, [session.id])

  useEffect(() => {
    if (!pendingAsset) return
    addSelectedAttachments([pendingAsset])
    onAssetConsumed?.()
  }, [pendingAsset, onAssetConsumed, addSelectedAttachments])

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

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!value.trim() && !selection.attachments.length) return
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
    onSend(value, selection.attachments, goalArmed, goalArmed ? goalTokenBudget : null)
    requestTranscriptBottom()
    setValue('')
    setGoalArmed(false)
    if (promptRef.current) promptRef.current.style.height = 'auto'
    selection.clearAttachments()
  }

  return (
    <Panel className="focus-session">
      <div className="card-head">
        <div className="session-runtime-meta">
          <button
            className="workspace-chip"
            title={cwd}
            onClick={onWorkspace}
            disabled={streaming || switchingCwd}
          >
            <FolderOpen size={11} />
            {workspaceName(cwd, language)}
          </button>
        </div>
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

      <FocusTranscript
        sessionId={session.id}
        messages={messages}
        messageStart={messageStart}
        hasOlder={hasOlder}
        loadingOlder={loadingOlder}
        olderError={olderError}
        plan={plan}
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
        onLoadOlder={onLoadOlder}
        onPromptSelect={applyWelcomeChip}
      />

      <form className="focus-composer-shell" onSubmit={submit}>
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
          <button
            type="button"
            className="attach-trigger"
            title={t('chat:focusSession.addAttachment')}
            aria-label={t('chat:focusSession.addAttachment')}
            onClick={() => selection.inputRef.current?.click()}
            disabled={streaming}
          >
            <Paperclip size={17} />
            {selection.attachments.length > 0 && <i>{selection.attachments.length}</i>}
          </button>
          <input
            ref={selection.inputRef}
            className="sr-only"
            type="file"
            multiple
            accept="image/*,.txt,.md,.json,.js,.jsx,.ts,.tsx,.css,.html,.xml,.yaml,.yml,.csv,.log,.py,.java,.go,.rs,.sh,.ps1,.toml,.sql,.pdf,.docx,.pptx,.xlsx,.odt,.odp,.ods,.rtf,.epub"
            onChange={selection.chooseFiles}
          />
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
          <div className="focus-composer-tools">
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
          </div>
          <div className="focus-composer-secondary">
            <ContextUsageIndicator
              usage={contextUsage}
              onThresholdChange={onCompactionThresholdChange}
            />
          </div>
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
          <button
            type={streaming ? 'button' : 'submit'}
            className={`send-button${streaming ? ' stop' : ''}`}
            title={streaming ? t('chat:focusSession.stop') : t('chat:focusSession.sendMessage')}
            aria-label={
              streaming ? t('chat:focusSession.stop') : t('chat:focusSession.sendMessage')
            }
            onClick={streaming ? onAbort : undefined}
            disabled={!streaming && (queueing || (!value.trim() && !selection.attachments.length))}
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
      </form>
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
              {attachment.kind === 'image'
                ? t('chat:focusSession.image')
                : attachment.kind === 'document'
                  ? t('chat:focusSession.document')
                  : t('chat:focusSession.text')}{' '}
              · {formatFileSize(attachment.size)}
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
