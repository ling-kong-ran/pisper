// 聚焦会话视图：单会话沉浸式聊天页（大输入框 + 完整转录）。
// 拆分说明：props 类型在 focus-session-props，composer 状态与提交逻辑在
// use-focus-composer，输入区小组件（排队托盘/资源芯片/状态灯/按钮）在
// focus-session-composer-bits，状态文案在 focus-session-status；
// composer 主体与发送行为约定保留在本文件。
import { lazy, memo, Suspense, useEffect, useId, useMemo, useRef, useState } from 'react'
import {
  AudioLines,
  Braces,
  Command,
  FolderOpen,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  SunMoon,
  SlidersHorizontal,
  TerminalSquare,
  Pencil,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { WorkbenchSidebarToggle } from '@/components/layout/WorkbenchSidebarToggle'
import { useUiStore } from '@/stores/ui-store'
import { AppCard as Panel, AppCardHeader } from '@/components/ui/app-primitives'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { workspaceName } from '@/lib/format'
import { cn } from '@/lib/utils'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { AttachmentPicker } from './AttachmentPicker'
import { AttachmentTray } from './AttachmentTray'
import { ChatResourcePicker } from './ChatResourcePicker'
import { ComposerCommandMenu } from './ComposerCommandMenu'
import { commandDraft, useComposerDraft } from './composer-drafts'
import {
  allocateComposerToolbar,
  COMPOSER_TOOL_IDS,
  type ComposerToolId,
} from './composer-toolbar-layout'
import { ComposerToolTray } from './ComposerToolTray'
import { ComposerToolbarSettings } from './ComposerToolbarSettings'
import { ChatRequestNotice } from './ChatRequestNotice'
import { requestCommandPalette } from './events'
import {
  ApprovalModeSelect as ExecutionModeSelect,
  ContextUsageIndicator,
  ModelThinkingControl,
  SessionUsageMetrics,
} from './FocusRuntimeControls'
import { FocusTranscript } from './FocusTranscript'
import { useChatLayoutStore } from './layout/chat-layout-store'
import { ChatCanvasLayout, ChatCanvasSlot } from './layout/ChatCanvasLayout'
import { canvasHasKind, createDefaultCanvas, parseChatCanvas } from './layout/chat-canvas'
import {
  CHAT_LAYOUT_ACCENT_CLASS,
  chatLayoutAppearanceStyle,
  chatLayoutMeasurementKey,
} from './layout/chat-layout-appearance'
import { ExecutionModeControl } from './GoalModeControl'
import { SessionTreeControl } from './SessionTreeControl'
import { SessionWorkflowRuns } from './SessionWorkflowRuns'
import { ToolApproval } from './ToolApproval'
import { VisualComposerEntry } from './VisualComposerEntry'
import { VoiceInputControl } from './VoiceInputControl'
import { VoiceModeOverlay } from './VoiceModeOverlay'
import {
  CompactContextButton,
  ComposerResourceChip,
  ComposerSendButton,
  ComposerStatusPill,
  QueuedInputsTray,
} from '@/features/chat/focus-session-composer-bits'
import type { FocusSessionProps } from '@/features/chat/focus-session-props'
import { useFocusSessionStatusLabel } from '@/features/chat/focus-session-status'
import { useComposerToolbarCapacity } from '@/features/chat/use-composer-toolbar-capacity'
import { useFocusComposer } from '@/features/chat/use-focus-composer'
import { useComposerToolbarStore } from '@/features/chat/composer-toolbar-store'
import { useShortcutStore } from '@/stores/shortcut-store'
import { matchesShortcut, shortcutEventBlocked, useShortcutLabel } from '@/lib/shortcuts'

export type { FocusSessionProps }

const QuickPromptIdeas = lazy(() => import('./QuickPromptIdeas'))

const CanvasSessionContext = lazy(() =>
  import('./layout/CanvasSessionContext').then((module) => ({
    default: module.CanvasSessionContext,
  })),
)

// 托盘外部点击关闭的排除区域：触发按钮、托盘壳、锚定弹层，以及 portal 到 body 的
// 对话框与浮层（Git diff 对话框、Radix Dialog/AlertDialog/Popover/DropdownMenu/Select/Sheet）。
// 缺了它们时，点击浮层会被当成外部点击而收起托盘，托盘卸载又连带销毁面板状态
//（典型场景：审阅 diff 时切换文件，整个 diff 对话框直接消失）。
const TRAY_CHILD_FLOATING_SELECTOR = [
  '.anchored-popup-menu',
  '.permission-mode-menu',
  '.voice-input-popup',
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
const TRAY_FLOATING_SELECTOR = [
  '.composer-tools-trigger',
  '.composer-tool-tray-shell',
  TRAY_CHILD_FLOATING_SELECTOR,
].join(', ')

export const FocusSession = memo(function FocusSession({
  session,
  shortcutEnabled = false,
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
  withdrawingInputIds,
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
  runCompleted = false,
  runNotice,
  approvals,
  error,
  pendingAsset,
  contextOpen,
  contextCompact,
  contextPanelId,
  onToggleContext,
  notify,
  requestConfirm,
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
  onRetryLastTurn,
  onTreeNavigated,
  onSend,
  onQueue,
  onWithdrawQueuedInput,
  onAbort,
}: FocusSessionProps) {
  const { t, language } = useI18n()
  const cycleTheme = useUiStore((state) => state.cycleTheme)
  const shortcuts = useShortcutStore((state) => state.bindings)
  const COMMAND_PALETTE_SHORTCUT = useShortcutLabel('commandPalette')
  const mobileApp = useIsMobileApp()
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  const chatLayout = useChatLayoutStore((state) => state.active)
  const appearance = mobileLayout ? chatLayout.mobile : chatLayout.desktop
  const layoutMeasurementKey = useMemo(
    () => `${chatLayoutMeasurementKey(appearance)}:${JSON.stringify(appearance.canvas)}`,
    [appearance],
  )
  const canvasModel = canvasHasKind(appearance.canvas, 'model')
  const canvasTools = canvasHasKind(appearance.canvas, 'tools')
  const canvasUsage = canvasHasKind(appearance.canvas, 'usage')
  const canvasContext = canvasHasKind(appearance.canvas, 'context')
  const canvasContextId = useId()
  const [canvasContextOpen, setCanvasContextOpen] = useState(true)
  const visibleContextOpen = canvasContext ? canvasContextOpen : contextOpen
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const goalsAvailable = runtimeFeatureAvailable(capabilities, 'goals')
  const teamAvailable = runtimeFeatureAvailable(capabilities, 'multiAgent')
  const plansAvailable = runtimeFeatureAvailable(capabilities, 'plans')
  const workflowsAvailable = runtimeFeatureAvailable(capabilities, 'workflows')
  const visualAvailable = runtimeFeatureAvailable(capabilities, 'visualGeneration')
  const { value, updateValue, selection, clearDraft, restoreDraft } = useComposerDraft(session.id)
  const [resourcePickerOpen, setResourcePickerOpen] = useState(false)
  const [sessionTreeOpen, setSessionTreeOpen] = useState(false)
  const [voiceModeOpen, setVoiceModeOpen] = useState(false)
  const [toolsOpen, setToolsOpen] = useState(false)
  const [detailsOpen, setDetailsOpen] = useState(false)
  const [scrollRequest, setScrollRequest] = useState(0)
  const contextOpenAtPointerDownRef = useRef<boolean | null>(null)
  const addSelectedAttachments = selection.addAttachments
  const promptRef = useRef<HTMLTextAreaElement>(null)
  // 语音实时转写与草稿的合并基准：录音开始时记住原文本，部分转写反复覆盖增量段。
  const voiceDraftBaseRef = useRef<string | null>(null)
  useEffect(() => {
    if (!shortcutEnabled) return
    const focusComposer = (event: KeyboardEvent) => {
      if (!shortcutEventBlocked(event) && matchesShortcut(event, shortcuts.voiceInput))
        setDetailsOpen(true)
      if (!shortcutEventBlocked(event) && matchesShortcut(event, shortcuts.focusComposer)) {
        event.preventDefault()
        promptRef.current?.focus()
      }
    }
    window.addEventListener('keydown', focusComposer)
    return () => window.removeEventListener('keydown', focusComposer)
  }, [shortcutEnabled, shortcuts.focusComposer, shortcuts.voiceInput])
  const toolbarRef = useRef<HTMLDivElement>(null)
  const toolTrayAnchorRef = useRef<HTMLButtonElement>(null)
  const toolTrayMenuRef = useRef<HTMLDivElement>(null)
  const toolbarCapacity = useComposerToolbarCapacity(toolbarRef)
  const toolbarLayout = useComposerToolbarStore((state) => state.layout)
  // 输入法组词跟踪：Mac WebKit 的确认 Enter 在 compositionend 后才派发，需自行跟踪并延迟复位。
  const imeComposingRef = useRef(false)
  const hasConversation = transcriptLoadState !== 'ready' || messages.length > 0
  // 只调整未自定义的默认空白画布；用户拖拽、CSS 和置顶输入布局保持原样。
  const centeredWelcome =
    !hasConversation &&
    appearance.composerPosition === 'bottom' &&
    [true, false].some(
      (includeIsland) =>
        JSON.stringify(appearance.canvas) ===
        JSON.stringify(parseChatCanvas(createDefaultCanvas({ ...appearance, includeIsland }))),
    )
  const composerStatusLabel = useFocusSessionStatusLabel(
    {
      streaming,
      messages,
      currentActivity,
      thinkingText,
      compaction,
      error,
      runStopped,
      runNotice,
      lastActivityAt,
    },
    t,
  )
  const toolTrayId = `composer-tool-tray-${session.id}`
  const quickActionsLabel = toolsOpen
    ? t('chat:focusSession.collapseQuickActions')
    : t('chat:focusSession.expandQuickActions')
  const composerPlaceholder = mobileLayout
    ? streaming
      ? t('chat:focusSession.addGuidanceForTheRunningAgent')
      : t('chat:workbench.composerPlaceholder')
    : streaming
      ? t('chat:focusSession.runningAgentComposerHint')
      : t('chat:focusSession.composerHint')

  const requestTranscriptBottom = () => setScrollRequest((current) => current + 1)

  // 语音文本写入草稿并自适应高度：value 可能是闭包旧值，以 textarea 实际值为准。
  const applyVoiceDraft = (text: string) => {
    updateValue(text)
    requestAnimationFrame(() => {
      const element = promptRef.current
      if (!element) return
      element.style.height = 'auto'
      element.style.height = `${Math.min(element.scrollHeight, 220)}px`
    })
  }

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
  const latestAgentMessage = messages.at(-1)?.role === 'agent' ? messages.at(-1) : null
  const currentError = error || (!streaming ? String(latestAgentMessage?.error || '') : '')
  const canRetryLastTurn = !streaming && messages.some((message) => message.role === 'user')

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
      if (event.key !== 'Escape' || event.defaultPrevented) return
      // Escape 先交给托盘中的设置或工具弹层，避免卸载子弹层并丢失返回焦点。
      if (document.querySelector(TRAY_CHILD_FLOATING_SELECTOR)) return
      setToolsOpen(false)
      toolTrayAnchorRef.current?.focus()
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

  const withdrawQueuedInput = async (inputId: string) => {
    const restored = await onWithdrawQueuedInput?.(inputId)
    if (!restored || !restoreDraft(restored)) return
    const element = promptRef.current
    requestAnimationFrame(() => {
      if (!element || promptRef.current !== element || !element.isConnected) return
      element.focus()
      element.style.height = 'auto'
      element.style.height = `${Math.min(element.scrollHeight, 220)}px`
    })
  }

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
  const composerToolLabels: Record<ComposerToolId, string> = {
    attachment: t('chat:focusSession.addAttachment'),
    resource: t('chat:resourcePicker.open'),
    visual: t('chat:focusSession.generateImage'),
    model: t('chat:focusSession.modelAndThinking'),
    permission: t('chat:focusSession.approvalMode'),
    'run-mode': t('chat:focusSession.executionMode'),
    commands: t('chat:focusSession.commands'),
    'compact-context': t('chat:focusSession.compactContextNow'),
  }
  const composerTools = {
    attachment: <AttachmentPicker cwd={cwd} selection={selection} />,
    resource: (
      <button
        type="button"
        className="resource-picker-trigger grid size-9 min-w-9 place-items-center rounded-[var(--r-sm)] border-0 bg-[var(--surface-subtle)] text-[var(--text-muted)] hover:bg-[var(--surface-hover)] hover:text-[var(--star-strong)]"
        title={t('chat:resourcePicker.open')}
        aria-label={t('chat:resourcePicker.open')}
        onClick={() => setResourcePickerOpen(true)}
      >
        <Braces size={16} />
      </button>
    ),
    visual: visualAvailable ? (
      <VisualComposerEntry
        notify={notify}
        onOpenModelSettings={onOpenModelSettings}
        onInsertPrompt={applyWelcomeChip}
      />
    ) : null,
    model: (
      <ModelThinkingControl
        model={model}
        models={availableModels}
        onModelChange={onModelChange}
        thinkingLevel={thinkingLevel || 'medium'}
        levels={availableThinkingLevels || []}
        status={thinkingStatus}
        message={thinkingMessage}
        onThinkingChange={onThinkingLevelChange}
        modelDisabled={streaming || switchingModel || switchingThinking}
        thinkingDisabled={streaming || switchingThinking || switchingModel}
      />
    ),
    permission: (
      <ExecutionModeSelect
        showLabel
        value={executionMode}
        onChange={onExecutionModeChange}
        disabled={switchingPermission}
      />
    ),
    'run-mode': goalsAvailable ? (
      <ExecutionModeControl
        mode={composerExecutionMode}
        goal={goal}
        teamAvailable={teamAvailable}
        disabled={streaming}
        tokenBudget={composerExecutionMode === 'team' ? teamTokenBudget : goalTokenBudget}
        onTokenBudgetChange={
          composerExecutionMode === 'team' ? setTeamTokenBudget : setGoalTokenBudget
        }
        onSaveTokenBudget={(tokenBudget) => onGoalBudgetChange?.(tokenBudget)}
        onChange={(nextMode) => {
          if (goal?.status === 'active' && (nextMode === 'plan' || nextMode !== goal.mode))
            void requestGoalPause().catch(() => {})
          setComposerExecutionMode(nextMode)
          void onRunModeChange(nextMode)
        }}
      />
    ) : null,
    commands: (
      <button
        type="button"
        className="command-palette-trigger relative grid size-9 min-w-9 place-items-center rounded-[var(--r-sm)] border-0 bg-[var(--surface-subtle)] text-[var(--text-muted)] hover:bg-[var(--surface-hover)] hover:text-[var(--star-strong)] [&_kbd]:sr-only"
        title={
          COMMAND_PALETTE_SHORTCUT
            ? t('chat:focusSession.openCommandPaletteShortcut', {
                shortcut: COMMAND_PALETTE_SHORTCUT,
              })
            : t('chat:focusSession.commands')
        }
        aria-label={
          COMMAND_PALETTE_SHORTCUT
            ? t('chat:focusSession.openCommandPaletteShortcut', {
                shortcut: COMMAND_PALETTE_SHORTCUT,
              })
            : t('chat:focusSession.commands')
        }
        onClick={requestCommandPalette}
      >
        <Command size={16} />
        <kbd>{COMMAND_PALETTE_SHORTCUT}</kbd>
      </button>
    ),
    'compact-context': (
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
    ),
  }
  const availableComposerToolIds = COMPOSER_TOOL_IDS.filter(
    (id) => composerTools[id] !== null && !(id === 'model' && canvasModel),
  )
  const toolbarAllocation = allocateComposerToolbar(
    toolbarLayout,
    availableComposerToolIds,
    // Respect the actual composer width; small layouts may move secondary settings into +.
    Math.max(toolbarCapacity, mobileLayout ? 5 : 8.5),
    mobileLayout
      ? { permission: 2.3, 'run-mode': 1.8, model: 3.6 }
      : { permission: 2.6, 'run-mode': 2, model: 3.9 },
  )
  const renderComposerTool = (id: ComposerToolId) => (
    <div
      className={cn(
        'composer-toolbar-slot flex min-w-0 flex-none items-center gap-2',
        id === 'model' && 'ml-auto [.composer-tool-tray_&]:ml-0',
        !['model', 'permission', 'run-mode'].includes(id) &&
          'h-10 [&>button]:!size-10 [&>div]:!size-10 [&>div>button]:!size-10',
      )}
      data-composer-tool-id={id}
      key={id}
    >
      {id === 'model' ? <ChatCanvasSlot kind="model" /> : composerTools[id]}
      {!['model', 'permission', 'run-mode'].includes(id) && (
        <span className="hidden text-xs text-muted-foreground [.composer-tool-tray_&]:block">
          {composerToolLabels[id]}
        </span>
      )}
    </div>
  )
  const workspaceBlock = (
    <button
      type="button"
      className="header-workspace inline-flex h-8 min-w-0 max-w-[150px] shrink items-center gap-1.5 rounded-md px-1.5 text-xs text-muted-foreground hover:bg-muted disabled:opacity-50 @max-[520px]:size-8 @max-[520px]:shrink-0 @max-[520px]:justify-center @max-[520px]:[&_span]:hidden"
      title={cwd}
      aria-label={t('chat:focusSession.changeWorkingDirectoryWorkspace', {
        workspace: workspaceName(cwd, language),
      })}
      onClick={onWorkspace}
      disabled={streaming || switchingCwd}
    >
      <FolderOpen size={12} />
      <span className="truncate">{workspaceName(cwd, language)}</span>
    </button>
  )
  const headerBlock = (
    <AppCardHeader
      data-window-drag-region
      data-context-aside={(visibleContextOpen && !contextCompact && !canvasContext) || undefined}
      className={cn(
        'workbench-chat-header relative z-4 h-12 shrink-0 items-center gap-2 border-0 bg-transparent px-4 py-1',
        window.pisperDesktop?.platform === 'darwin' && 'pl-[74px]',
      )}
    >
      <WorkbenchSidebarToggle />
      <div className="flex min-w-0 flex-1 items-center gap-1" data-window-drag-region>
        {workspaceBlock}
        <span className="text-muted-foreground/40" aria-hidden="true">
          /
        </span>
        <button
          type="button"
          className="group inline-flex min-w-0 max-w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-[13px] font-medium hover:bg-muted"
          title={t('chat:focusSession.renameChat')}
          aria-label={t('chat:focusSession.renameChat')}
          onClick={onRename}
        >
          <span className="truncate">{session.name || t('navigation:pageHeader.newChat')}</span>
          <Pencil
            size={12}
            aria-hidden="true"
            className="shrink-0 opacity-0 group-hover:opacity-60 group-focus-visible:opacity-60"
          />
        </button>
      </div>
      <div className="flex flex-none items-center gap-1">
        {window.pisperDesktop?.terminalProfiles &&
          runtimeFeatureAvailable(capabilities, 'terminal') && (
            <button
              type="button"
              className="grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-muted"
              aria-label={t('navigation:pageHeader.toggleTerminal')}
              title={t('navigation:pageHeader.toggleTerminal')}
              onClick={() => window.dispatchEvent(new Event('pisper:toggle-terminal'))}
            >
              <TerminalSquare size={16} />
            </button>
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
        <button
          type="button"
          className="grid size-8 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground"
          aria-label={
            visibleContextOpen ? t('chat:sessionContext.close') : t('chat:sessionContext.open')
          }
          title={
            visibleContextOpen ? t('chat:sessionContext.close') : t('chat:sessionContext.open')
          }
          aria-expanded={visibleContextOpen}
          aria-controls={canvasContext ? canvasContextId : contextPanelId}
          aria-haspopup={!canvasContext && contextCompact ? 'dialog' : undefined}
          onPointerDown={() => {
            // 指针点击其他 Dock 面板时，焦点会先切换活动会话；保留点击时看到的开关状态。
            contextOpenAtPointerDownRef.current = contextOpen
          }}
          onPointerCancel={() => {
            contextOpenAtPointerDownRef.current = null
          }}
          onClick={() => {
            if (canvasContext) {
              setCanvasContextOpen((open) => !open)
              contextOpenAtPointerDownRef.current = null
              return
            }
            const wasOpen = contextOpenAtPointerDownRef.current ?? contextOpen
            contextOpenAtPointerDownRef.current = null
            onToggleContext(!wasOpen)
          }}
        >
          {visibleContextOpen ? <PanelRightClose size={16} /> : <PanelRightOpen size={16} />}
        </button>
        <button
          type="button"
          className="grid size-8 place-items-center rounded-lg text-muted-foreground hover:bg-muted"
          aria-label={t('navigation:workbench.changeTheme')}
          title={t('navigation:workbench.changeTheme')}
          onClick={cycleTheme}
        >
          <SunMoon size={16} />
        </button>
      </div>
    </AppCardHeader>
  )
  const transcriptBlock = (
    <FocusTranscript
      key="transcript"
      layoutMeasurementKey={layoutMeasurementKey}
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
      onRetryLastTurn={onRetryLastTurn}
      onPromptSelect={applyWelcomeChip}
      onWorkspace={onWorkspace}
    />
  )
  const toolsBlock = (
    <div
      ref={toolbarRef}
      className="focus-composer-quick-actions flex min-w-0 flex-1 items-end gap-1"
    >
      <button
        ref={toolTrayAnchorRef}
        type="button"
        className={`composer-tools-trigger grid size-9 min-w-9 max-[650px]:w-11 max-[650px]:min-w-11 h-10 flex-none place-items-center rounded-lg border-0 bg-transparent text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground ${toolsOpen ? 'active bg-foreground/5 text-foreground' : ''}`}
        title={quickActionsLabel}
        aria-label={quickActionsLabel}
        aria-expanded={toolsOpen}
        aria-controls={toolTrayId}
        onClick={() => setToolsOpen((open) => !open)}
      >
        {toolsOpen ? <X size={17} /> : <Plus size={18} />}
      </button>
      <div className="focus-composer-visible-tools flex min-w-0 flex-1 flex-wrap items-center gap-0.5">
        {toolbarAllocation.inline.map(renderComposerTool)}
      </div>
      <ComposerToolTray
        placement={appearance.composerPosition === 'top' ? 'bottom' : 'top'}
        open={toolsOpen}
        label={t('chat:focusSession.quickActions')}
        trayId={toolTrayId}
        anchorRef={toolTrayAnchorRef}
        menuRef={toolTrayMenuRef}
      >
        {toolbarAllocation.overflow.map(renderComposerTool)}
        {toolsOpen && (
          <Suspense fallback={null}>
            <QuickPromptIdeas
              plansAvailable={plansAvailable}
              onPromptSelect={(prompt) => {
                applyWelcomeChip(prompt)
                setToolsOpen(false)
              }}
            />
          </Suspense>
        )}
        <button
          type="button"
          className="flex h-9 w-full items-center gap-2 rounded-md px-2 text-xs text-muted-foreground hover:bg-muted"
          aria-expanded={detailsOpen}
          onClick={() => {
            setDetailsOpen((open) => !open)
            setToolsOpen(false)
          }}
        >
          <SlidersHorizontal size={15} />
          {detailsOpen ? t('chat:focusSession.hideDetails') : t('chat:focusSession.details')}
        </button>
        <div className="w-full border-t border-[var(--stroke-soft)] pt-1">
          <ComposerToolbarSettings labels={composerToolLabels} labeled />
        </div>
      </ComposerToolTray>
    </div>
  )
  const usageBlock = (
    <SessionUsageMetrics
      usage={sessionUsage}
      plan={plansAvailable ? plan : null}
      compact={mobileLayout}
    />
  )
  const composerBlock = (
    <form
      key="composer"
      className="focus-composer-shell relative z-20 mx-auto flex w-[min(600px,calc(100%_-_40px))] shrink-0 flex-col gap-2 pt-2 pb-4 @max-[700px]:w-[calc(100%_-_24px)]"
      onSubmit={submit}
    >
      <ToolApproval approvals={approvals} onResolve={onApproval} />
      {queuedInputs.length > 0 && (
        <QueuedInputsTray
          queuedInputs={queuedInputs}
          withdrawingInputIds={withdrawingInputIds}
          onWithdraw={onWithdrawQueuedInput ? withdrawQueuedInput : undefined}
        />
      )}
      {workflowsAvailable && <SessionWorkflowRuns sessionId={session.id} />}
      {invocation && (
        <ComposerResourceChip invocation={invocation} onRemove={() => setInvocation(null)} />
      )}
      <AttachmentTray attachments={selection.attachments} onRemove={selection.removeAttachment} />
      {selection.attachmentError && (
        <span className="text-[var(--danger)] text-[13px]">{selection.attachmentError}</span>
      )}
      {currentError && (
        <ChatRequestNotice
          error={currentError}
          onRetry={canRetryLastTurn ? onRetryLastTurn : undefined}
          className="w-full"
        />
      )}
      <ComposerStatusPill
        compaction={compaction}
        streaming={streaming}
        statusLabel={composerStatusLabel}
      />
      <div className="focus-composer relative flex min-w-0 flex-col gap-2 rounded-[18px] border border-border/70 bg-muted/45 p-2.5 shadow-xs transition-[border-color,box-shadow] focus-within:border-ring/50 focus-within:shadow-md dark:bg-[#242424] [&_textarea]:w-full [&_textarea]:min-w-0 [&_textarea]:min-h-[40px] [&_textarea]:max-h-[220px] [&_textarea]:resize-none [&_textarea]:overflow-y-auto [&_textarea]:border-0 [&_textarea]:[outline:0]! [&_textarea]:bg-transparent [&_textarea]:px-1 [&_textarea]:py-1.5 [&_textarea]:text-[length:var(--app-message-font-size)] [&_textarea]:font-normal [&_textarea]:leading-relaxed [&_textarea]:text-foreground [&_textarea]:placeholder:text-muted-foreground">
        <ComposerCommandMenu
          placement={appearance.composerPosition === 'top' ? 'bottom' : 'top'}
          sessionId={session.id}
          value={value}
          onChange={updateValue}
          inputRef={promptRef}
        />
        <textarea
          ref={promptRef}
          rows={1}
          aria-label={t('chat:focusSession.taskDescription')}
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
            // 保留换行与中英文组词，只在当前发送绑定精确匹配时提交。
            const composing = event.nativeEvent.isComposing || imeComposingRef.current
            if (!composing && matchesShortcut(event.nativeEvent, shortcuts.sendMessage)) {
              event.preventDefault()
              event.currentTarget.form?.requestSubmit()
            }
          }}
          data-mobile-composer-input={mobileLayout || undefined}
          enterKeyHint={mobileLayout && shortcuts.sendMessage === 'Enter' ? 'send' : 'enter'}
          placeholder={composerPlaceholder}
        />
        <div className="focus-composer-footer flex min-w-0 items-end gap-1">
          {!canvasTools ? <ChatCanvasSlot kind="tools" /> : <div className="min-w-0 flex-1" />}
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
      <div
        className={cn(
          'composer-details flex min-h-9 min-w-0 flex-wrap items-center gap-2 px-2 py-1 text-xs text-muted-foreground',
          !detailsOpen && '!hidden',
        )}
      >
        {hasConversation && appearance.showUsage && !canvasUsage && <ChatCanvasSlot kind="usage" />}
        <button
          type="button"
          className="grid size-7 place-items-center rounded-md hover:bg-muted"
          aria-label={t('chat:focusSession.hideDetails')}
          onClick={() => setDetailsOpen(false)}
        >
          <X size={14} />
        </button>
        <div className="ml-auto flex items-center gap-1 [&_.voice-input-control_button]:!size-8">
          <div className="focus-composer-secondary flex h-8 min-w-0 flex-none items-center justify-end">
            <ContextUsageIndicator
              usage={contextUsage}
              sessionUsage={sessionUsage}
              model={model}
              availableModels={availableModels}
              onThresholdChange={onCompactionThresholdChange}
              compact
            />
          </div>
          <>
            <button
              type="button"
              className="grid !size-8 !min-w-8 place-items-center rounded-[var(--r-sm)] border border-transparent bg-[var(--surface-subtle)] text-[var(--text-muted)] transition-[background-color,color,border-color,box-shadow,transform] duration-200 hover:scale-105 hover:border-[var(--brand-blue)] hover:bg-[var(--brand-blue-soft)] hover:text-[var(--brand-blue-strong)]"
              title={t('chat:voiceMode.open')}
              aria-label={t('chat:voiceMode.open')}
              onClick={() => setVoiceModeOpen(true)}
            >
              <AudioLines size={17} />
            </button>
            <VoiceInputControl
              sessionId={session.id}
              shortcutEnabled={shortcutEnabled}
              onLiveText={(text) => {
                if (text === null) {
                  // 录音取消/失败：回滚到录音前的草稿。
                  if (voiceDraftBaseRef.current !== null) {
                    applyVoiceDraft(voiceDraftBaseRef.current)
                    voiceDraftBaseRef.current = null
                  }
                  return
                }
                if (voiceDraftBaseRef.current === null) {
                  voiceDraftBaseRef.current = (promptRef.current?.value ?? value).trimEnd()
                }
                const base = voiceDraftBaseRef.current
                applyVoiceDraft(base && text ? `${base}\n${text}` : base || text)
              }}
              onInsert={(transcript) => {
                // 最终转写替换实时部分文本（基准段 + 终稿）。
                const base =
                  voiceDraftBaseRef.current ?? (promptRef.current?.value ?? value).trimEnd()
                voiceDraftBaseRef.current = null
                applyVoiceDraft(base ? `${base}\n${transcript}` : transcript)
                requestAnimationFrame(() => promptRef.current?.focus())
              }}
            />
          </>
        </div>
      </div>
    </form>
  )
  return (
    <Panel
      className={cn(
        `focus-session [.session-dock-panel_&]:overflow-hidden [.session-dock-panel_&]:min-h-0 [.session-dock-panel_&]:border-0 [.session-dock-panel_&]:rounded-[0] [.session-dock-panel_&]:bg-[var(--panel)] [.session-dock-panel_&]:p-0 [.session-dock-panel_&]:shadow-[none] [[data-theme='dark']_.session-dock-panel_&]:bg-[var(--main-surface-bg)] min-[651px]:[[data-density='compact']_.app-card:not(&)]:p-[10px] max-[650px]:min-h-[460px] max-[650px]:[.session-dock-panel_&]:min-h-0 relative flex h-full min-h-0 flex-col ${hasConversation ? 'has-conversation' : 'is-empty'}`,
        centeredWelcome &&
          '[&_[data-canvas-node=canvas-messages]]:!flex-none [&_[data-canvas-node=canvas-messages]]:mt-[max(48px,calc((100dvh-560px)/2))] [&_[data-canvas-node=canvas-messages]]:h-[160px] [&_[data-canvas-node=canvas-messages]]:!overflow-visible [&_.transcript]:!overflow-visible [&_.transcript]:!p-0 max-[650px]:[&_[data-canvas-node=canvas-messages]]:mt-8 max-[650px]:[&_[data-canvas-node=canvas-messages]]:h-[150px]',
        chatLayout.accent !== 'inherit' && CHAT_LAYOUT_ACCENT_CLASS,
      )}
      style={chatLayoutAppearanceStyle(appearance, chatLayout.accent)}
      data-chat-message-style={appearance.messageStyle}
      data-chat-density={appearance.density}
      data-chat-composer-position={appearance.composerPosition}
    >
      <ChatCanvasLayout
        root={appearance.canvas}
        notify={notify}
        slots={{
          header: headerBlock,
          messages: transcriptBlock,
          composer: composerBlock,
          model: composerTools.model,
          tools: toolsBlock,
          usage: usageBlock,
          workspace: null,
          context: canvasContext ? (
            <Suspense fallback={<div aria-busy="true" className="min-h-12" />}>
              <CanvasSessionContext
                panelId={canvasContextId}
                sessionId={session.id}
                plan={plan ?? null}
                streaming={Boolean(streaming)}
                completed={runCompleted}
                plansAvailable={plansAvailable}
                requestConfirm={requestConfirm}
                open={canvasContextOpen}
                autoOpen={appearance.openContextOnCompletion}
                onOpenChange={setCanvasContextOpen}
              />
            </Suspense>
          ) : null,
        }}
      />
      <ChatResourcePicker
        open={resourcePickerOpen}
        sessionId={session.id}
        onClose={() => setResourcePickerOpen(false)}
        onSelect={setInvocation}
        onCommandSelect={(commandInvocation) =>
          applyWelcomeChip(commandDraft(commandInvocation, value))
        }
      />
      <VoiceModeOverlay
        open={voiceModeOpen}
        sessionId={session.id}
        sessionName={session.name || session.id}
        messages={messages}
        streaming={streaming}
        sendPrompt={onSend}
        onAbort={onAbort}
        onClose={() => setVoiceModeOpen(false)}
      />
    </Panel>
  )
})
