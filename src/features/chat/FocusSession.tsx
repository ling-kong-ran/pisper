// 聚焦会话视图：单会话沉浸式聊天页（大输入框 + 完整转录）。
import { memo, useEffect, useRef, useState, type FormEvent } from 'react'
import {
  Braces,
  Command,
  FolderOpen,
  Minimize2,
  Plus,
  RefreshCw,
  Send,
  Square,
  Wrench,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { QueueSection } from '@/components/ai-elements/queue'
import { AppCard as Panel, AppCardHeader } from '@/components/ui/app-primitives'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { workspaceName } from '@/lib/format'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import type { Notify } from '@/app/route-context'
import type {
  ChatAttachment,
  ChatMessage,
  EntityRecord,
  ModelOption,
  Plan,
  ResourceInvocation,
  SessionSummary,
} from '@/types/chat'
import { AttachmentPicker } from './AttachmentPicker'
import { AttachmentTray } from './AttachmentTray'
import { ChatResourcePicker } from './ChatResourcePicker'
import { commandDraft, ComposerCommandMenu } from './ComposerCommandMenu'
import { useComposerDraft } from './composer-drafts'
import { ComposerToolTray } from './ComposerToolTray'
import { requestCommandPalette } from './events'
import {
  ApprovalModeSelect as ExecutionModeSelect,
  ContextUsageIndicator,
  SessionModelSelect,
  SessionThinkingSelect,
  SessionUsageMetrics,
} from './FocusRuntimeControls'
import { FocusTranscript, type TranscriptLoadState } from './FocusTranscript'
import { GitChangesControl } from './GitChangesControl'
import { ExecutionModeControl } from './GoalModeControl'
import { SessionActionsMenu } from './SessionActionsMenu'
import { SessionTreeControl } from './SessionTreeControl'
import { SessionWorkflowRuns } from './SessionWorkflowRuns'
import { ToolApproval } from './ToolApproval'
import { VisualComposerEntry } from './VisualComposerEntry'
const USES_COMMAND_KEY = /Mac|iPhone|iPad/.test(globalThis.navigator?.platform || '')
const COMMAND_PALETTE_SHORTCUT = USES_COMMAND_KEY ? '\u2318 K' : 'Ctrl K'
export type FocusSessionProps = {
  session: SessionSummary
  messages: ChatMessage[]
  transcriptLoadState?: TranscriptLoadState
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
  team?: EntityRecord | null
  plan?: Plan | null
  currentActivity?: EntityRecord | null
  activityFeed: EntityRecord[]
  tools: EntityRecord[]
  thinkingText?: string
  queuedInputs: EntityRecord[]
  compaction?: EntityRecord | null
  contextUsage?: EntityRecord | null
  sessionUsage?: EntityRecord | null
  sessionTreeRevision?: number
  sessionTreePulse?: number
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
  // 是否提供「关闭面板」入口：移动端单会话视图没有可关闭的面板。
  canClosePanel?: boolean
  notify?: Notify
  onOpenModelSettings?: () => void
  onAssetConsumed?: () => void
  onLoadOlder?: () => Promise<boolean> | boolean
  onModelChange: (model: string) => Promise<void> | void
  onThinkingLevelChange: (level: string) => Promise<void> | void
  onExecutionModeChange: (mode: string) => Promise<boolean> | boolean
  onGoalPause?: () => Promise<void> | void
  onGoalBudgetChange?: (tokenBudget: number | null) => Promise<void> | void
  onCompact?: () => Promise<void> | void
  onCompactionThresholdChange?: (thresholdPercent: number) => Promise<void> | void
  onApproval: (approvalId: string, approved: boolean) => Promise<void> | void
  onWorkspace: () => void
  onRename: () => void
  onBranchFromHere: (boundaryEntryId: string) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
  onTreeNavigated?: () => Promise<void> | void
  onSplitLeft: () => void
  onSplitRight: () => void
  onSplitTop: () => void
  onSplitBottom: () => void
  onClosePanel: () => void
  onSend: (
    value: string,
    attachments: ChatAttachment[],
    goalMode: boolean,
    teamMode: boolean,
    goalTokenBudget: number | null,
    invocation?: ResourceInvocation | null,
  ) => Promise<void> | void
  onQueue?: (
    value: string,
    attachments: ChatAttachment[],
    behavior: string,
  ) => Promise<boolean> | boolean
  onAbort: () => Promise<void> | void
}
export const FocusSession = memo(function FocusSession({
  session,
  messages,
  transcriptLoadState = 'ready',
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
  team,
  plan,
  currentActivity,
  activityFeed,
  tools,
  thinkingText,
  queuedInputs,
  compaction,
  contextUsage,
  sessionUsage,
  sessionTreeRevision,
  sessionTreePulse,
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
  canClosePanel = true,
  notify,
  onOpenModelSettings,
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
  onBranchFromHere,
  onCreateChildSession,
  onTreeNavigated,
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
  const mobileApp = useIsMobileApp()
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const goalsAvailable = runtimeFeatureAvailable(capabilities, 'goals')
  const teamAvailable = runtimeFeatureAvailable(capabilities, 'multiAgent')
  const plansAvailable = runtimeFeatureAvailable(capabilities, 'plans')
  const vcsAvailable = runtimeFeatureAvailable(capabilities, 'vcs')
  const workflowsAvailable = runtimeFeatureAvailable(capabilities, 'workflows')
  const visualAvailable = runtimeFeatureAvailable(capabilities, 'visualGeneration')
  const { value, updateValue, selection, clearDraft } = useComposerDraft(session.id)
  const [composerExecutionMode, setComposerExecutionMode] = useState<'plan' | 'goal' | 'team'>(
    'plan',
  )
  const [goalTokenBudget, setGoalTokenBudget] = useState<number | null>(null)
  const [teamTokenBudget, setTeamTokenBudget] = useState<number | null>(null)
  const goalPausePromiseRef = useRef<Promise<void> | null>(null)
  const [queueing, setQueueing] = useState(false)
  const [compactingManually, setCompactingManually] = useState(false)
  const [scrollRequest, setScrollRequest] = useState(0)
  const [resourcePickerOpen, setResourcePickerOpen] = useState(false)
  const [sessionTreeOpen, setSessionTreeOpen] = useState(false)
  const [toolsOpen, setToolsOpen] = useState(false)
  const [invocation, setInvocation] = useState<ResourceInvocation | null>(null)
  const addSelectedAttachments = selection.addAttachments
  const promptRef = useRef<HTMLTextAreaElement>(null)
  // 输入法组词跟踪：Mac WebKit 的确认 Enter 在 compositionend 后才派发，需自行跟踪并延迟复位。
  const imeComposingRef = useRef(false)
  const hasConversation = transcriptLoadState !== 'ready' || messages.length > 0
  const toolTrayId = `composer-tool-tray-${session.id}`
  const quickActionsLabel = toolsOpen
    ? t('chat:focusSession.collapseQuickActions')
    : t('chat:focusSession.expandQuickActions')
  const composerPlaceholder = mobileLayout
    ? streaming
      ? t('chat:focusSession.addGuidanceForTheRunningAgent')
      : t('chat:focusSession.writeWhatYouWantToAccomplish')
    : streaming
      ? t('chat:focusSession.runningAgentComposerHint')
      : t('chat:focusSession.composerHint')
  useEffect(() => {
    setComposerExecutionMode('plan')
    goalPausePromiseRef.current = null
    setGoalTokenBudget(null)
    setTeamTokenBudget(null)
    setQueueing(false)
    setCompactingManually(false)
    setInvocation(null)
    setResourcePickerOpen(false)
    setSessionTreeOpen(false)
    setToolsOpen(false)
  }, [session.id])
  useEffect(() => {
    if (!goalsAvailable) setComposerExecutionMode('plan')
    if (!teamAvailable && composerExecutionMode === 'team') setComposerExecutionMode('goal')
    if (!workflowsAvailable && invocation?.kind === 'workflow') setInvocation(null)
  }, [composerExecutionMode, goalsAvailable, invocation?.kind, teamAvailable, workflowsAvailable])
  useEffect(() => {
    if (goal?.status === 'active')
      setComposerExecutionMode(goal.mode === 'team' && teamAvailable ? 'team' : 'goal')
  }, [goal?.id, goal?.mode, goal?.status, teamAvailable])
  useEffect(() => {
    if (!goal?.id) {
      setGoalTokenBudget(null)
      return
    }
    const rawBudget = goal.mode === 'team' ? goal.teamTokenBudget : goal.tokenBudget
    const savedBudget = rawBudget == null ? null : Number(rawBudget)
    const nextBudget =
      savedBudget !== null && Number.isFinite(savedBudget) && savedBudget > 0 ? savedBudget : null
    if (goal.mode === 'team') setTeamTokenBudget(nextBudget)
    else setGoalTokenBudget(nextBudget)
  }, [goal?.id, goal?.mode, goal?.teamTokenBudget, goal?.tokenBudget])
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
    const closeOnPointerDown = (event: PointerEvent) => {
      const target = event.target
      if (
        target instanceof Element &&
        !target.closest('.composer-tools-trigger, .composer-tool-tray-shell')
      )
        setToolsOpen(false)
    }
    document.addEventListener('keydown', close)
    document.addEventListener('pointerdown', closeOnPointerDown)
    return () => {
      document.removeEventListener('keydown', close)
      document.removeEventListener('pointerdown', closeOnPointerDown)
    }
  }, [toolsOpen])
  const applyWelcomeChip = (prompt: string) => {
    updateValue(prompt)
    requestAnimationFrame(() => {
      const element = promptRef.current
      if (!element) return
      element.focus()
      element.style.height = 'auto'
      element.style.height = `${Math.min(element.scrollHeight, 220)}px`
    })
  }
  const requestTranscriptBottom = () => setScrollRequest((current) => current + 1)
  const requestGoalPause = () => {
    if (goalPausePromiseRef.current) return goalPausePromiseRef.current
    const pending = Promise.resolve().then(() => onGoalPause?.())
    goalPausePromiseRef.current = pending
    void pending.catch(() => {})
    return pending
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
  // 提交输入：空输入不发送；流式中走排队，否则发送目标模式/资源命令并清空草稿。
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!value.trim() && !selection.attachments.length && !invocation) return
    if (streaming) {
      if ((!value.trim() && !selection.attachments.length) || queueing) return
      setQueueing(true)
      const queued = await onQueue?.(value, selection.attachments, 'steer')
      setQueueing(false)
      if (!queued) return
      clearDraft()
      setComposerExecutionMode('plan')
      setToolsOpen(false)
      requestTranscriptBottom()
      if (promptRef.current) promptRef.current.style.height = 'auto'
      return
    }
    const pendingGoalPause = goalPausePromiseRef.current
    if (pendingGoalPause) {
      try {
        await pendingGoalPause
      } catch {
        goalPausePromiseRef.current = null
        return
      }
      goalPausePromiseRef.current = null
    } else if (
      goal?.status === 'active' &&
      (composerExecutionMode === 'plan' || composerExecutionMode !== goal.mode)
    )
      await onGoalPause?.()
    onSend(
      value,
      selection.attachments,
      composerExecutionMode === 'goal' || composerExecutionMode === 'team',
      composerExecutionMode === 'team',
      composerExecutionMode === 'team'
        ? teamTokenBudget
        : composerExecutionMode === 'goal'
          ? goalTokenBudget
          : null,
      invocation,
    )
    requestTranscriptBottom()
    clearDraft()
    setComposerExecutionMode('plan')
    setToolsOpen(false)
    setInvocation(null)
    if (promptRef.current) promptRef.current.style.height = 'auto'
  }
  const composerLeadingTools = (
    <>
      <AttachmentPicker cwd={cwd} selection={selection} />
      <button
        type="button"
        className="resource-picker-trigger [.focus-composer_&]:h-[38px] [.focus-composer_&]:border-0 [.focus-composer_&]:rounded-[var(--r-sm)] [.focus-composer_&]:bg-[var(--surface-subtle)] [.focus-composer_&]:text-[12px] [.focus-composer_&]:w-[38px] [.focus-composer_&]:min-w-[38px] [.focus-session.has-conversation_.focus-composer_&]:w-[36px] [.focus-session.has-conversation_.focus-composer_&]:min-w-[36px] [.focus-session.has-conversation_.focus-composer_&]:h-[36px] relative grid place-items-center border-0 rounded-[var(--r-xs)] bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--star-strong)]"
        title={t('chat:resourcePicker.open')}
        aria-label={t('chat:resourcePicker.open')}
        onClick={() => setResourcePickerOpen(true)}
      >
        <Braces size={16} />
      </button>
      {visualAvailable && (
        <VisualComposerEntry
          notify={notify}
          onOpenModelSettings={onOpenModelSettings}
          onInsertPrompt={applyWelcomeChip}
        />
      )}
    </>
  )
  // 空会话头部与会话中 composer 角落共用同一份会话操作菜单。
  const sessionActionsMenu = (
    <SessionActionsMenu
      session={session}
      canSplit={canSplit}
      canClose={canClosePanel}
      streaming={streaming}
      switchingCwd={switchingCwd}
      onSplitLeft={onSplitLeft}
      onSplitRight={onSplitRight}
      onSplitTop={onSplitTop}
      onSplitBottom={onSplitBottom}
      onClosePanel={onClosePanel}
      onWorkspace={onWorkspace}
      onRename={onRename}
      onSessionTree={() => setSessionTreeOpen(true)}
    />
  )
  return (
    <Panel
      className={`focus-session [.session-dock-panel_&]:overflow-hidden [.session-dock-panel_&]:min-h-0 [.session-dock-panel_&]:border-0 [.session-dock-panel_&]:rounded-[0] [.session-dock-panel_&]:bg-[var(--panel)] [.session-dock-panel_&]:p-0 [.session-dock-panel_&]:shadow-[none] [[data-theme='dark']_.session-dock-panel_&]:bg-[var(--main-surface-bg)] min-[651px]:[[data-density='compact']_.app-card:not(&)]:p-[10px] max-[650px]:min-h-[460px] max-[650px]:[.session-dock-panel_&]:min-h-0 relative flex h-full min-h-[500px] flex-col ${hasConversation ? 'has-conversation' : 'is-empty'}`}
    >
      {!hasConversation && (
        <AppCardHeader className="relative z-4 min-h-12 flex-none items-center border-b border-[var(--stroke-soft)] bg-[var(--panel)] px-3 py-[7px]">
          <div className="flex [margin-left:auto] items-center gap-[6px]">{sessionActionsMenu}</div>
        </AppCardHeader>
      )}
      <SessionTreeControl
        visible={messages.length > 0}
        open={sessionTreeOpen}
        sessionId={session.id}
        streaming={Boolean(streaming)}
        revision={sessionTreeRevision}
        pulseToken={sessionTreePulse}
        onOpenChange={setSessionTreeOpen}
        onNavigated={async (editorText) => {
          if (editorText !== null) applyWelcomeChip(editorText)
          await onTreeNavigated?.()
          requestTranscriptBottom()
        }}
        onCreateChildSession={(entryId) => onCreateChildSession(entryId)}
      />
      <FocusTranscript
        sessionId={session.id}
        messages={messages}
        transcriptLoadState={transcriptLoadState}
        messageStart={messageStart}
        hasOlder={hasOlder}
        loadingOlder={loadingOlder}
        olderError={olderError}
        currentActivity={currentActivity}
        team={team}
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
        onBranchFromHere={onBranchFromHere}
        onCreateChildSession={onCreateChildSession}
        onPromptSelect={applyWelcomeChip}
        onWorkspace={onWorkspace}
      />
      <form
        className="focus-composer-shell [.focus-session.has-conversation_&]:w-[min(900px,calc(100%_-_48px))] [.focus-session.has-conversation_&]:pt-[8px] @max-[700px]:w-[calc(100%_-_20px)] @max-[700px]:pb-[10px] @max-[700px]:[.focus-session.has-conversation_&]:w-[calc(100%_-_20px)] max-[650px]:w-[calc(100%_-_20px)] max-[650px]:pb-[10px] relative z-20 flex w-[min(960px,calc(100%_-_48px))] flex-none flex-col gap-[7px] [margin:0_auto] [padding:10px_0_0]"
        onSubmit={submit}
      >
        <ToolApproval approvals={approvals} onResolve={onApproval} />
        {streaming && queuedInputs.length > 0 && (
          <QueueSection asChild defaultOpen>
            <div
              className="queued-input-tray [&_>_span]:flex-none [&_>_span]:text-[var(--star-strong)] [&_>_span]:font-[600] [&_>_small]:max-w-[240px] [&_>_small]:overflow-hidden [&_>_small]:rounded-[var(--r-pill)] [&_>_small]:bg-[var(--surface-muted)] [&_>_small]:p-[4px_8px] [&_>_small]:text-[var(--text-secondary)] [&_>_small]:text-ellipsis [&_>_small]:whitespace-nowrap [&_>_em]:flex-none [&_>_em]:[font-style:normal] flex min-w-0 items-center gap-[6px] overflow-hidden text-[var(--text-muted)] text-[11px]"
              data-pisper-queue-size={queuedInputs.length}
            >
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
        {workflowsAvailable && <SessionWorkflowRuns sessionId={session.id} />}
        {invocation && (
          <div
            className={`composer-resource-chip [&.workflow]:border-[var(--success)] [&.workflow]:bg-[var(--success-soft)] [&_button]:grid [&_button]:w-[20px] [&_button]:h-[20px] [&_button]:place-items-center [&_button]:border-0 [&_button]:rounded-[var(--r-xs)] [&_button]:bg-transparent [&_button]:text-[var(--text-muted)] [&_button]:cursor-pointer [&_button:hover]:bg-[var(--surface-hover)] [&_button:hover]:text-[var(--text)] inline-flex min-h-[28px] self-start items-center gap-[6px] [border:1px_solid_var(--blue)] rounded-[var(--r-sm)] bg-[var(--blue-soft)] text-[var(--text)] [padding:4px_6px_4px_8px] text-[12px] font-[600] ${invocation.kind}`}
          >
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
          <span className="text-[var(--danger)] text-[13px]">{selection.attachmentError}</span>
        )}
        <div
          className={`focus-composer-status [.focus-session.has-conversation_&.idle]:hidden [&_>_i]:w-[7px] [&_>_i]:h-[7px] [&_>_i]:flex-none [&_>_i]:rounded-[50%] [&_>_i]:bg-[var(--text-muted)] [&.running]:text-[var(--success-strong)] [&.running_>_i]:bg-[var(--success)] [&.running_>_i]:shadow-[0_0_0_3px_var(--success-soft)] [&.running_>_i]:[animation:star-twinkle_1.1s_ease-in-out_infinite] inline-flex min-h-[22px] self-start items-center gap-[7px] [margin:0_0_-2px_5px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-pill)] bg-[var(--solid)] [padding:3px_9px_3px_7px] text-[var(--text-muted)] text-[11px] font-[600] shadow-[0_8px_18px_-16px_var(--shadow-strong)] ${compaction?.active ? 'compacting [.focus-composer-status&]:text-[var(--warning-strong)] [.focus-composer-status&_>_i]:bg-[var(--warning-strong)] [.focus-composer-status&_>_i]:shadow-[0_0_0_3px_var(--warning-soft)] [.focus-composer-status&_>_i]:[animation:star-twinkle_1.1s_ease-in-out_infinite]' : streaming ? 'running' : 'idle'}`}
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
        <div className="focus-composer [&:focus-within]:border-[var(--focus)] [&:focus-within]:shadow-[0_0_0_3px_var(--focus-ring)] [&_textarea]:w-full [&_textarea]:min-w-0 [&_textarea]:min-h-[48px] [&_textarea]:max-h-[220px] [&_textarea]:[align-self:start] [&_textarea]:resize-none [&_textarea]:overflow-y-auto [&_textarea]:border-0 [&_textarea]:[outline:0]! [&_textarea]:bg-transparent [&_textarea]:p-[5px_6px_8px] [&_textarea]:text-[var(--text)] [&_textarea]:text-[14px] [&_textarea]:leading-[1.5] [.focus-session.has-conversation_&]:shadow-[0_10px_28px_-24px_var(--shadow-strong)] [.focus-session.has-conversation_&_textarea]:min-h-[50px] [.focus-session.has-conversation_&_textarea]:p-[6px_7px_8px] dark:bg-[var(--solid)] dark:text-[var(--text)] @max-[700px]:grid-cols-[70px_36px_36px_auto_minmax(0,1fr)_36px] @max-[700px]:grid-rows-[minmax(48px,1fr)_36px] relative flex min-w-0 flex-col items-stretch gap-[4px] [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)] [padding:8px] shadow-[0_14px_34px_-24px_var(--shadow-strong)] [transition:border-color_var(--d1)_var(--ease-out),_box-shadow_var(--d2)_var(--ease-out)]">
          <ComposerCommandMenu
            sessionId={session.id}
            value={value}
            onChange={updateValue}
            inputRef={promptRef}
          />
          <textarea
            ref={promptRef}
            rows={1}
            value={value}
            onChange={(event) => {
              updateValue(event.target.value)
              event.currentTarget.style.height = 'auto'
              event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 220)}px`
            }}
            onPaste={selection.pasteFiles}
            onCompositionStart={() => (imeComposingRef.current = true)}
            onCompositionEnd={() => window.setTimeout(() => (imeComposingRef.current = false), 0)}
            onKeyDown={(event) => {
              // Enter 发送、Shift+Enter 换行；兼容 Chromium 与 Mac WebKit 的 composition 事件。
              const composing = event.nativeEvent.isComposing || imeComposingRef.current
              if (event.key === 'Enter' && !event.shiftKey && !composing) {
                event.preventDefault()
                event.currentTarget.form?.requestSubmit()
              }
            }}
            data-mobile-composer-input={mobileLayout || undefined}
            enterKeyHint={mobileLayout ? 'send' : 'enter'}
            placeholder={composerPlaceholder}
          />
          <div className="focus-composer-footer flex min-w-0 items-center gap-1">
            <div className="focus-composer-quick-actions flex min-w-0 flex-none items-center gap-1">
              <button
                type="button"
                className={`composer-tools-trigger grid !size-11 !min-w-11 place-items-center rounded-[var(--r-sm)] border border-transparent bg-[var(--surface-subtle)] text-[var(--text-muted)] cursor-pointer transition-[transform,background-color,color,border-color,box-shadow] duration-200 ease-[var(--ease-spring)] hover:scale-105 hover:border-[var(--brand-blue)] hover:bg-[var(--brand-blue-soft)] hover:text-[var(--brand-blue-strong)] ${toolsOpen ? 'active rotate-90 scale-105 border-[var(--brand-blue)] bg-[var(--brand-blue-soft)] text-[var(--brand-blue-strong)] shadow-[0_0_18px_-5px_var(--brand-blue)]' : ''}`}
                title={quickActionsLabel}
                aria-label={quickActionsLabel}
                aria-expanded={toolsOpen}
                aria-controls={toolTrayId}
                onClick={() => setToolsOpen((open) => !open)}
              >
                {toolsOpen ? <X size={17} /> : <Plus size={18} />}
              </button>
              <div className="focus-composer-visible-tools flex min-w-0 flex-none items-center gap-1">
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
                {goalsAvailable && (
                  <ExecutionModeControl
                    mode={composerExecutionMode}
                    goal={goal}
                    teamAvailable={teamAvailable}
                    tokenBudget={
                      composerExecutionMode === 'team' ? teamTokenBudget : goalTokenBudget
                    }
                    onTokenBudgetChange={
                      composerExecutionMode === 'team' ? setTeamTokenBudget : setGoalTokenBudget
                    }
                    onSaveTokenBudget={(tokenBudget) => onGoalBudgetChange?.(tokenBudget)}
                    onChange={(nextMode) => {
                      if (
                        goal?.status === 'active' &&
                        (nextMode === 'plan' || nextMode !== goal.mode)
                      )
                        void requestGoalPause().catch(() => {})
                      setComposerExecutionMode(nextMode)
                    }}
                  />
                )}
                <SessionThinkingSelect
                  value={thinkingLevel || 'medium'}
                  levels={availableThinkingLevels || []}
                  status={thinkingStatus}
                  message={thinkingMessage}
                  onChange={onThinkingLevelChange}
                  disabled={streaming || switchingThinking || switchingModel}
                />
              </div>
              <ComposerToolTray
                open={toolsOpen}
                label={t('chat:focusSession.quickActions')}
                trayId={toolTrayId}
              >
                {composerLeadingTools}
                <button
                  type="button"
                  className="command-palette-trigger [.focus-composer_&]:h-[38px] [.focus-composer_&]:border-0 [.focus-composer_&]:rounded-[var(--r-sm)] [.focus-composer_&]:bg-[var(--surface-subtle)] [.focus-composer_&]:text-[12px] [.composer-tool-tray_&]:relative [.composer-tool-tray_&]:grid [.composer-tool-tray_&]:w-[38px] [.composer-tool-tray_&]:min-w-[38px] [.composer-tool-tray_&]:place-items-center [.composer-tool-tray_&]:p-0 [.composer-tool-tray_&]:text-[var(--text-muted)] [.composer-tool-tray_&]:cursor-pointer [.composer-tool-tray_&:hover]:bg-[var(--surface-hover)] [.composer-tool-tray_&:hover]:text-[var(--star-strong)] [.composer-tool-tray_&_kbd]:absolute [.composer-tool-tray_&_kbd]:w-[1px] [.composer-tool-tray_&_kbd]:h-[1px] [.composer-tool-tray_&_kbd]:overflow-hidden [.composer-tool-tray_&_kbd]:[clip:rect(0_0_0_0)] [.composer-tool-tray_&_kbd]:[clip-path:inset(50%)] [.composer-tool-tray_&_kbd]:whitespace-nowrap @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:min-w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[700px]:[.composer-tool-tray_&]:p-0 @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:min-w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px]"
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
                {vcsAvailable && <GitChangesControl sessionId={session.id} streaming={streaming} />}
                <button
                  type="button"
                  className="compact-context-trigger [&:hover:not(:disabled)]:border-[var(--accent-border)] [&:hover:not(:disabled)]:bg-[var(--accent-soft)] [&:hover:not(:disabled)]:text-[var(--star-strong)] disabled:[cursor:not-allowed] disabled:opacity-[.5] [.composer-tool-tray_&]:w-[38px] [.composer-tool-tray_&]:min-w-[38px] [.composer-tool-tray_&]:h-[38px] [.composer-tool-tray_&]:flex-none @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:min-w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[700px]:[.composer-tool-tray_&]:p-0 @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:min-w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px] grid w-[38px] h-[38px] flex-none place-items-center [border:1px_solid_transparent] rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-[var(--text-tertiary)] cursor-pointer"
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
                    <RefreshCw className="animate-spin" size={14} />
                  ) : (
                    <Minimize2 size={14} />
                  )}
                </button>
                {hasConversation && (
                  <div className="focus-composer-session-actions [.composer-tool-tray_&]:w-[38px] [.composer-tool-tray_&]:min-w-[38px] [.composer-tool-tray_&]:h-[38px] [.composer-tool-tray_&]:flex-none @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:min-w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[700px]:[.composer-tool-tray_&]:p-0 @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:min-w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px]">
                    {sessionActionsMenu}
                  </div>
                )}
              </ComposerToolTray>
            </div>
            <div className="focus-composer-secondary flex h-11 min-w-0 flex-1 items-center justify-end">
              <ContextUsageIndicator
                usage={contextUsage}
                onThresholdChange={onCompactionThresholdChange}
                compact
              />
            </div>
            <button
              type={streaming ? 'button' : 'submit'}
              className={`send-button grid !size-11 flex-none place-items-center rounded-[var(--r-sm)] border-0 bg-[var(--star)] text-[var(--on-accent)] transition-[var(--d1)] cursor-pointer hover:not(:disabled):bg-[var(--star-hover)] hover:not(:disabled):shadow-[var(--sh-star)] active:not(:disabled):scale-[.96] disabled:cursor-not-allowed disabled:border disabled:border-[var(--stroke)] disabled:bg-[var(--surface-muted)] disabled:text-[var(--text-muted)] ${streaming ? 'stop !bg-[var(--danger)] hover:not(:disabled):shadow-[0_0_0_3px_var(--danger-soft)]' : ''}`}
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
                <RefreshCw className="animate-spin" size={17} />
              ) : (
                <Send size={18} />
              )}
            </button>
          </div>
        </div>
        <div className="flex min-w-0 min-h-[26px] items-center gap-[6px] [margin:0_8px_5px] text-[var(--text-tertiary)]">
          {hasConversation && (
            <button
              type="button"
              className="composer-workspace-status [&_span]:min-w-0 [&_span]:overflow-hidden [&_span]:text-ellipsis [&_span]:whitespace-nowrap [&:hover:not(:disabled)]:text-[var(--star-strong)] disabled:opacity-[.55] disabled:[cursor:not-allowed] @max-[470px]:w-[24px] @max-[470px]:p-0 @max-[470px]:justify-center @max-[470px]:[&_span]:hidden inline-flex max-w-[180px] min-w-0 h-[24px] flex-none items-center gap-[4px] overflow-hidden border-0 rounded-[0] bg-transparent [padding:2px_0] text-inherit text-[10px] cursor-pointer"
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
          <SessionUsageMetrics
            usage={sessionUsage}
            plan={plansAvailable ? plan : null}
            compact={mobileLayout}
          />
        </div>
      </form>
      <ChatResourcePicker
        open={resourcePickerOpen}
        sessionId={session.id}
        onClose={() => setResourcePickerOpen(false)}
        onSelect={setInvocation}
        onCommandSelect={(commandInvocation) =>
          applyWelcomeChip(commandDraft(commandInvocation, value))
        }
      />
    </Panel>
  )
})
