// 聚焦会话视图：单会话沉浸式聊天页（大输入框 + 完整转录）。
// 拆分说明：props 类型在 focus-session-props，composer 状态与提交逻辑在
// use-focus-composer，输入区小组件（排队托盘/资源芯片/状态灯/按钮）在
// focus-session-composer-bits；composer 主体与发送行为约定保留在本文件。
import { memo, useEffect, useRef, useState } from 'react'
import { Braces, Command, FolderOpen, Plus, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppCard as Panel, AppCardHeader } from '@/components/ui/app-primitives'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { workspaceName } from '@/lib/format'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { AttachmentPicker } from './AttachmentPicker'
import { AttachmentTray } from './AttachmentTray'
import { ChatResourcePicker } from './ChatResourcePicker'
import { ComposerCommandMenu } from './ComposerCommandMenu'
import { commandDraft, useComposerDraft } from './composer-drafts'
import { ComposerToolTray } from './ComposerToolTray'
import { requestCommandPalette } from './events'
import {
  ApprovalModeSelect as ExecutionModeSelect,
  ContextUsageIndicator,
  SessionModelSelect,
  SessionThinkingSelect,
  SessionUsageMetrics,
} from './FocusRuntimeControls'
import { FocusTranscript } from './FocusTranscript'
import { GitChangesControl } from './GitChangesControl'
import { ExecutionModeControl } from './GoalModeControl'
import { SessionActionsMenu } from './SessionActionsMenu'
import { SessionTreeControl } from './SessionTreeControl'
import { SessionWorkflowRuns } from './SessionWorkflowRuns'
import { ToolApproval } from './ToolApproval'
import { VisualComposerEntry } from './VisualComposerEntry'
import {
  CompactContextButton,
  ComposerResourceChip,
  ComposerSendButton,
  ComposerStatusPill,
  QueuedInputsTray,
} from '@/features/chat/focus-session-composer-bits'
import type { FocusSessionProps } from '@/features/chat/focus-session-props'
import { useFocusComposer } from '@/features/chat/use-focus-composer'

export type { FocusSessionProps }

const USES_COMMAND_KEY = /Mac|iPhone|iPad/.test(globalThis.navigator?.platform || '')
const COMMAND_PALETTE_SHORTCUT = USES_COMMAND_KEY ? '\u2318 K' : 'Ctrl K'

// 托盘外部点击关闭的排除区域：触发按钮、托盘壳、锚定弹层，以及 portal 到 body 的
// 对话框与浮层（Git diff 对话框、Radix Dialog/AlertDialog/Popover/DropdownMenu/Select/Sheet）。
// 缺了它们时，点击浮层会被当成外部点击而收起托盘，托盘卸载又连带销毁面板状态
//（典型场景：审阅 diff 时切换文件，整个 diff 对话框直接消失）。
const TRAY_FLOATING_SELECTOR = [
  '.composer-tools-trigger',
  '.composer-tool-tray-shell',
  '.anchored-popup-menu',
  '.git-diff-dialog-backdrop',
  "[data-slot='dialog-content']",
  "[data-slot='dialog-overlay']",
  "[data-slot='alert-dialog-content']",
  "[data-slot='alert-dialog-overlay']",
  "[data-slot='popover-content']",
  "[data-slot='dropdown-menu-content']",
  "[data-slot='dropdown-menu-sub-content']",
  "[data-slot='select-content']",
  "[data-slot='sheet-content']",
  "[data-slot='sheet-overlay']",
].join(', ')

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
  onRunModeChange,
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
  const [resourcePickerOpen, setResourcePickerOpen] = useState(false)
  const [sessionTreeOpen, setSessionTreeOpen] = useState(false)
  const [toolsOpen, setToolsOpen] = useState(false)
  const [scrollRequest, setScrollRequest] = useState(0)
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

  const requestTranscriptBottom = () => setScrollRequest((current) => current + 1)

  const {
    composerExecutionMode,
    setComposerExecutionMode,
    goalTokenBudget,
    setGoalTokenBudget,
    teamTokenBudget,
    setTeamTokenBudget,
    queueing,
    compactingManually,
    invocation,
    setInvocation,
    requestGoalPause,
    compactContext,
    submit,
  } = useFocusComposer({
    sessionId: session.id,
    runMode: session.runMode,
    goalsAvailable,
    teamAvailable,
    workflowsAvailable,
    goal,
    streaming,
    compaction,
    value,
    selection,
    clearDraft,
    promptRef,
    onCompact,
    onGoalPause,
    onSend,
    onQueue,
    setToolsOpen,
    requestTranscriptBottom,
  })

  useEffect(() => {
    setResourcePickerOpen(false)
    setSessionTreeOpen(false)
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
    const closeOnPointerDown = (event: PointerEvent) => {
      const target = event.target
      // 点击托盘自身或其衍生的 portal 浮层（弹层菜单、diff/资源等对话框）时不收起托盘。
      if (target instanceof Element && !target.closest(TRAY_FLOATING_SELECTOR)) setToolsOpen(false)
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
        {streaming && queuedInputs.length > 0 && <QueuedInputsTray queuedInputs={queuedInputs} />}
        {workflowsAvailable && <SessionWorkflowRuns sessionId={session.id} />}
        {invocation && (
          <ComposerResourceChip invocation={invocation} onRemove={() => setInvocation(null)} />
        )}
        <AttachmentTray attachments={selection.attachments} onRemove={selection.removeAttachment} />
        {selection.attachmentError && (
          <span className="text-[var(--danger)] text-[13px]">{selection.attachmentError}</span>
        )}
        <ComposerStatusPill compaction={compaction} streaming={streaming} />
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
                      void onRunModeChange(nextMode)
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
                <CompactContextButton
                  streaming={streaming}
                  compactingManually={compactingManually}
                  compactionActive={Boolean(compaction?.active)}
                  disabled={
                    !onCompact ||
                    streaming ||
                    compactingManually ||
                    Boolean(compaction?.active) ||
                    messages.length === 0
                  }
                  onCompact={() => void compactContext()}
                />
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
            <ComposerSendButton
              streaming={streaming}
              queueing={queueing}
              disabled={
                !streaming &&
                (queueing || (!value.trim() && !selection.attachments.length && !invocation))
              }
              onAbort={onAbort}
            />
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
