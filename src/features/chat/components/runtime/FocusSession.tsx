// 聚焦会话视图：单会话沉浸式聊天页（大输入框 + 完整转录）。
// 拆分说明：props 类型在 focus-session-props，composer 状态与提交逻辑在
// use-focus-composer，输入区小组件（排队托盘/资源芯片/状态灯/按钮）在
// focus-session-composer-bits，状态文案在 focus-session-status；
// composer 主体与发送行为约定保留在本文件。
import {
  lazy,
  memo,
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
} from 'react'
import {
  AudioLines,
  Gauge,
  Braces,
  Command,
  ChevronDown,
  FolderOpen,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  SlidersHorizontal,
  TerminalSquare,
  Pencil,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { DesktopTitlebarPortal } from '@/components/layout/DesktopTitlebar'
import { SidebarStatusPortal } from '@/components/layout/SidebarStatus'
import { WorkbenchSidebarToggle } from '@/components/layout/WorkbenchSidebarToggle'
import { ThemeToggleButton } from '@/components/layout/ThemeToggleButton'
import { useUiStore } from '@/stores/ui-store'
import { AppCard as Panel, AppCardHeader } from '@/components/ui/app-primitives'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { workspaceName } from '@/lib/format/format'
import { cn } from '@/lib/utils'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { ComposerActionButton } from '@/features/chat/components/composer/ComposerActionButton'
import { AttachmentPicker } from '@/features/chat/components/composer/AttachmentPicker'
import { AttachmentTray } from '@/features/chat/components/composer/AttachmentTray'
import { ChatResourcePicker } from '@/features/chat/components/composer/ChatResourcePicker'
import { ComposerCommandMenu } from '@/features/chat/components/composer/ComposerCommandMenu'
import { commandDraft, useComposerDraft } from '@/features/chat/model/composer-drafts'
import {
  allocateComposerToolbar,
  COMPOSER_TOOL_IDS,
  type ComposerToolId,
} from '@/features/chat/model/composer-toolbar-layout'
import { ComposerToolTray } from '@/features/chat/components/composer/ComposerToolTray'
import { ComposerToolbarSettings } from '@/features/chat/components/composer/ComposerToolbarSettings'
import { ChatRequestNotice } from '@/features/chat/components/ChatRequestNotice'
import { requestCommandPalette } from '@/features/chat/model/events'
import {
  ApprovalModeSelect as ExecutionModeSelect,
  ContextUsageIndicator,
  ModelThinkingControl,
  SessionUsageMetrics,
} from '@/features/chat/components/runtime/FocusRuntimeControls'
import { FocusTranscript } from '@/features/chat/components/message/FocusTranscript'
import { ExecutionModeControl } from '@/features/chat/components/runtime/GoalModeControl'
import { SessionTreeControl } from '@/features/chat/components/session/SessionTreeControl'
import { SessionWorkflowRuns } from '@/features/chat/components/session/SessionWorkflowRuns'
import { ToolApproval } from '@/features/chat/components/runtime/ToolApproval'
import { VisualComposerEntry } from '@/features/chat/components/VisualComposerEntry'
import { VoiceInputControl } from '@/features/chat/components/voice/VoiceInputControl'
import { VoiceModeOverlay } from '@/features/chat/components/voice/VoiceModeOverlay'
import {
  ComposerResourceChip,
  ComposerSendButton,
  ComposerStatusPill,
  QueuedInputsTray,
} from '@/features/chat/model/focus-session-composer-bits'
import type { FocusSessionProps } from '@/features/chat/model/focus-session-props'
import { useFocusSessionStatusLabel } from '@/features/chat/model/focus-session-status'
import { useComposerToolbarCapacity } from '@/features/chat/hooks/use-composer-toolbar-capacity'
import { useSessionViewport } from '@/features/chat/hooks/useSessionViewport'
import { useFocusComposer } from '@/features/chat/hooks/use-focus-composer'
import { useComposerToolbarStore } from '@/features/chat/model/composer-toolbar-store'
import { useShortcutStore } from '@/stores/shortcut-store'
import { matchesShortcut, shortcutEventBlocked, useShortcutLabel } from '@/lib/ui/shortcuts'

export type { FocusSessionProps }

const QuickPromptIdeas = lazy(() => import('@/features/chat/components/composer/QuickPromptIdeas'))
const ComposerAddMenu = lazy(() =>
  import('@/features/chat/components/composer/ComposerAddMenu').then((module) => ({
    default: module.ComposerAddMenu,
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
  headerToolsTargetId,
  titlebarToolsActive = false,
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
  pendingRuntimeSelection,
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
  contextOpen,
  contextCompact,
  contextPanelId,
  onToggleContext,
  notify,
  onOpenModelSettings,
  onAssetConsumed,
  onLoadOlder,
  onModelChange,
  onThinkingLevelChange,
  onLoadThinkingLevel,
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
  const theme = useUiStore((state) => state.theme)
  const shortcuts = useShortcutStore((state) => state.bindings)
  const COMMAND_PALETTE_SHORTCUT = useShortcutLabel('commandPalette')
  const mobileApp = useIsMobileApp()
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  const sidebarCollapsed = useUiStore((state) => state.sidebarCollapsed)
  const voiceInputEnabled = useUiStore((state) => state.composerVoiceInput)
  const voiceConversationEnabled = useUiStore((state) => state.composerVoiceConversation)
  const showVoiceInput = mobileLayout || voiceInputEnabled
  const showVoiceConversation = mobileLayout || voiceConversationEnabled
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const goalsAvailable = runtimeFeatureAvailable(capabilities, 'goals')
  const teamAvailable = runtimeFeatureAvailable(capabilities, 'multiAgent')
  const plansAvailable = runtimeFeatureAvailable(capabilities, 'plans')
  const workflowsAvailable = runtimeFeatureAvailable(capabilities, 'workflows')
  const visualAvailable = runtimeFeatureAvailable(capabilities, 'visualGeneration')
  const { value, updateValue, selection, clearDraft, restoreDraft } = useComposerDraft(session.id)
  const [resourcePickerOpen, setResourcePickerOpen] = useState(false)
  const [resourceCategory, setResourceCategory] = useState<'all' | 'skill' | 'workflow'>('all')
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
      if (
        mobileLayout &&
        !shortcutEventBlocked(event) &&
        matchesShortcut(event, shortcuts.voiceInput)
      )
        setDetailsOpen(true)
      if (!shortcutEventBlocked(event) && matchesShortcut(event, shortcuts.focusComposer)) {
        event.preventDefault()
        promptRef.current?.focus()
      }
    }
    window.addEventListener('keydown', focusComposer)
    return () => window.removeEventListener('keydown', focusComposer)
  }, [mobileLayout, shortcutEnabled, shortcuts.focusComposer, shortcuts.voiceInput])
  const toolbarRef = useRef<HTMLDivElement>(null)
  const toolTrayAnchorRef = useRef<HTMLButtonElement>(null)
  const composerAnchorRef = useRef<HTMLDivElement>(null)
  const toolTrayMenuRef = useRef<HTMLDivElement>(null)
  const toolbarCapacity = useComposerToolbarCapacity(toolbarRef)
  const toolbarLayout = useComposerToolbarStore((state) => state.layout)
  // 输入法组词跟踪：Mac WebKit 的确认 Enter 在 compositionend 后才派发，需自行跟踪并延迟复位。
  const imeComposingRef = useRef(false)
  const hasConversation = transcriptLoadState !== 'ready' || messages.length > 0
  const sessionViewport = useSessionViewport()
  const titlebarTools = !mobileLayout && Boolean(window.pisperDesktop?.customTitlebar)
  const [transcriptGutter, setTranscriptGutter] = useState({ left: 0, right: 0 })
  const updateTranscriptGutter = useCallback((left: number, right: number) => {
    setTranscriptGutter((previous) =>
      previous.left === left && previous.right === right ? previous : { left, right },
    )
  }, [])
  const compactWelcome = !hasConversation && sessionViewport.compact
  const centeredWelcome = !hasConversation && !compactWelcome
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
    invocation,
    setInvocation,
    requestGoalPause,
    commandError,
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
    hasMessages: messages.length > 0,
    updateValue,
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
  const compactCommand = /^\/compact(?:\s|$)/.test(value.trim())
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
    attachment: (menu: boolean) => <AttachmentPicker cwd={cwd} selection={selection} menu={menu} />,
    resource: (menu: boolean) => (
      <ComposerActionButton
        menu={menu}
        label={t('chat:focusSession.resourcesAndTools')}
        className="resource-picker-trigger"
        title={t('chat:resourcePicker.open')}
        aria-label={t('chat:resourcePicker.open')}
        onClick={() => setResourcePickerOpen(true)}
      >
        <Braces size={16} />
      </ComposerActionButton>
    ),
    visual: visualAvailable
      ? (menu: boolean) => (
          <VisualComposerEntry
            menu={menu}
            notify={notify}
            onOpenModelSettings={onOpenModelSettings}
            onInsertPrompt={applyWelcomeChip}
          />
        )
      : null,
    model: () => (
      <ModelThinkingControl
        model={pendingRuntimeSelection?.model || model}
        models={availableModels}
        onModelChange={onModelChange}
        thinkingLevel={pendingRuntimeSelection?.thinkingLevel || thinkingLevel || 'medium'}
        levels={pendingRuntimeSelection?.model ? [] : availableThinkingLevels || []}
        status={thinkingStatus}
        message={thinkingMessage}
        onThinkingChange={onThinkingLevelChange}
        onRefreshThinking={onLoadThinkingLevel}
        modelDisabled={switchingModel || switchingThinking}
        thinkingDisabled={switchingThinking || switchingModel}
        deferred={streaming}
        pendingModel={Boolean(pendingRuntimeSelection?.model)}
      />
    ),
    permission: () => (
      <ExecutionModeSelect
        showLabel
        value={executionMode}
        onChange={onExecutionModeChange}
        disabled={switchingPermission}
      />
    ),
    'run-mode': goalsAvailable
      ? () => (
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
        )
      : null,
    commands: (menu: boolean) => (
      <ComposerActionButton
        menu={menu}
        label={t('chat:focusSession.commands')}
        className="command-palette-trigger"
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
      </ComposerActionButton>
    ),
    'compact-context': null,
  }
  const availableComposerToolIds = COMPOSER_TOOL_IDS.filter((id) => composerTools[id] !== null)
  const toolbarAllocation = allocateComposerToolbar(
    mobileLayout ? toolbarLayout : { inline: ['permission', 'run-mode', 'model'], overflow: [] },
    mobileLayout
      ? availableComposerToolIds
      : availableComposerToolIds.filter((id) => ['permission', 'run-mode', 'model'].includes(id)),
    // 分屏后的实际可用宽度决定常驻项，多余设置仍可从“+”打开。
    toolbarCapacity,
    mobileLayout
      ? { permission: 2.3, 'run-mode': 1.8, model: 3.6 }
      : { permission: 2.6, 'run-mode': 2, model: 3.9 },
  )
  const renderComposerTool = (id: ComposerToolId, menu = false) => (
    <div
      className={cn(
        'composer-toolbar-slot flex min-w-0 max-w-full flex-none items-center',
        menu && 'w-full [&>div]:w-full [&>div>button]:w-full [&>div>button]:justify-start',
        id === 'model' && 'ml-auto [.composer-tool-tray_&]:ml-0',
        !menu &&
          !['model', 'permission', 'run-mode'].includes(id) &&
          'h-10 [&>button]:!size-10 [&>div]:!size-10 [&>div>button]:!size-10',
      )}
      data-composer-tool-id={id}
      key={id}
    >
      {composerTools[id]?.(menu)}
    </div>
  )
  const workspaceBlock = (
    <button
      type="button"
      className={cn(
        'header-workspace inline-flex h-8 min-w-0 shrink items-center gap-1.5 rounded-md px-1.5 text-xs text-foreground hover:bg-muted disabled:opacity-50',
        mobileLayout
          ? 'max-w-[150px] @max-[520px]:size-8 @max-[520px]:shrink-0 @max-[520px]:justify-center @max-[520px]:[&_span]:hidden'
          : 'max-w-full',
      )}
      title={cwd}
      aria-label={t('chat:focusSession.changeWorkingDirectoryWorkspace', {
        workspace: cwd ? workspaceName(cwd, language) : t('chat:focusSession.noWorkspace'),
      })}
      onClick={onWorkspace}
      disabled={streaming || switchingCwd}
    >
      <FolderOpen size={12} />
      <span className="truncate">
        {cwd ? workspaceName(cwd, language) : t('chat:focusSession.noWorkspace')}
      </span>
      {!mobileLayout && (
        <ChevronDown size={12} className="shrink-0 text-muted-foreground" aria-hidden="true" />
      )}
    </button>
  )
  const headerActions = (
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
        aria-label={contextOpen ? t('chat:sessionContext.close') : t('chat:sessionContext.open')}
        title={contextOpen ? t('chat:sessionContext.close') : t('chat:sessionContext.open')}
        aria-expanded={contextOpen}
        aria-controls={contextPanelId}
        aria-haspopup={contextCompact ? 'dialog' : undefined}
        onPointerDown={() => {
          // 指针点击其他 Dock 面板时，焦点会先切换活动会话；保留点击时看到的开关状态。
          contextOpenAtPointerDownRef.current = contextOpen
        }}
        onPointerCancel={() => {
          contextOpenAtPointerDownRef.current = null
        }}
        onClick={() => {
          const wasOpen = contextOpenAtPointerDownRef.current ?? contextOpen
          contextOpenAtPointerDownRef.current = null
          onToggleContext(!wasOpen)
        }}
      >
        {contextOpen ? <PanelRightClose size={16} /> : <PanelRightOpen size={16} />}
      </button>
      {mobileLayout && <ThemeToggleButton compact theme={theme} onCycle={cycleTheme} />}
    </div>
  )
  const headerBlock = (
    <AppCardHeader
      data-window-drag-region
      data-context-aside={(contextOpen && !contextCompact) || undefined}
      className={cn(
        'workbench-chat-header relative z-4 h-12 shrink-0 items-center gap-2 border-0 bg-transparent px-4 py-1',
        window.pisperDesktop?.platform === 'darwin' && 'pl-[74px]',
      )}
    >
      <WorkbenchSidebarToggle />
      <div className="flex min-w-0 flex-1 items-center gap-1" data-window-drag-region>
        {mobileLayout && (
          <>
            {workspaceBlock}
            <span className="text-muted-foreground/40" aria-hidden="true">
              /
            </span>
          </>
        )}
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
      {!titlebarTools && headerActions}
    </AppCardHeader>
  )
  const transcriptBlock = (
    <FocusTranscript
      key="transcript"
      emptyState={compactWelcome ? false : undefined}
      layoutMeasurementKey="fixed"
      onScrollbarGutterChange={updateTranscriptGutter}
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
      className={cn(
        'focus-composer-quick-actions flex min-w-9 flex-1 items-end gap-1',
        !mobileLayout && 'basis-[calc(100%_-_108px)]',
      )}
    >
      <button
        ref={toolTrayAnchorRef}
        type="button"
        className={`composer-tools-trigger grid size-9 min-w-9 max-[650px]:w-11 max-[650px]:min-w-11 h-10 flex-none place-items-center rounded-lg border-0 bg-transparent text-foreground transition-colors hover:bg-foreground/5 hover:text-foreground ${toolsOpen ? 'active bg-foreground/5 text-foreground' : ''}`}
        title={quickActionsLabel}
        aria-label={quickActionsLabel}
        aria-expanded={toolsOpen}
        aria-controls={toolTrayId}
        onClick={() => setToolsOpen((open) => !open)}
      >
        {toolsOpen ? <X size={17} /> : <Plus size={18} />}
      </button>
      <div className="focus-composer-visible-tools flex min-w-0 flex-1 flex-wrap items-center gap-0.5">
        {toolbarAllocation.inline.map((id) => renderComposerTool(id))}
      </div>
      {mobileLayout ? (
        <ComposerToolTray
          placement="top"
          open={toolsOpen}
          label={t('chat:focusSession.quickActions')}
          trayId={toolTrayId}
          anchorRef={toolTrayAnchorRef}
          menuRef={toolTrayMenuRef}
        >
          {toolbarAllocation.overflow.map((id) => renderComposerTool(id, true))}
          <div className="my-1 border-t border-border/60" />
          <ComposerActionButton
            menu
            label={
              detailsOpen ? t('chat:focusSession.hideDetails') : t('chat:focusSession.details')
            }
            aria-expanded={detailsOpen}
            onClick={() => {
              setDetailsOpen((open) => !open)
              setToolsOpen(false)
            }}
          >
            <SlidersHorizontal size={15} />
          </ComposerActionButton>
          <ComposerToolbarSettings labels={composerToolLabels} labeled />
        </ComposerToolTray>
      ) : toolsOpen ? (
        <Suspense fallback={null}>
          <ComposerAddMenu
            open={toolsOpen}
            sessionId={session.id}
            anchorRef={composerAnchorRef}
            menuRef={toolTrayMenuRef}
            menuId={toolTrayId}
            attachment={
              <AttachmentPicker
                cwd={cwd}
                selection={selection}
                menu
                onAdded={() => {
                  setToolsOpen(false)
                  promptRef.current?.focus()
                }}
              />
            }
            directory={
              <AttachmentPicker
                cwd={cwd}
                selection={selection}
                menu
                directory
                onAdded={() => {
                  setToolsOpen(false)
                  promptRef.current?.focus()
                }}
              />
            }
            visual={
              visualAvailable && (
                <VisualComposerEntry
                  menu
                  notify={notify}
                  onOpenModelSettings={onOpenModelSettings}
                  onInsertPrompt={(prompt) => {
                    applyWelcomeChip(prompt)
                    setToolsOpen(false)
                  }}
                />
              )
            }
            settings={
              toolbarAllocation.overflow.length > 0
                ? toolbarAllocation.overflow.map((id) => renderComposerTool(id, true))
                : null
            }
            goalsAvailable={goalsAvailable}
            workflowsAvailable={workflowsAvailable}
            goalDisabled={Boolean(streaming)}
            onGoal={() => {
              if (goal?.status === 'active' && goal.mode !== 'goal')
                void requestGoalPause().catch(() => {})
              setComposerExecutionMode('goal')
              void onRunModeChange('goal')
              setToolsOpen(false)
              promptRef.current?.focus()
            }}
            onWorkflow={() => {
              setResourceCategory('workflow')
              setResourcePickerOpen(true)
              setToolsOpen(false)
            }}
            onSelect={setInvocation}
            onCommandSelect={(command) => updateValue(commandDraft(command, value))}
            onClose={() => {
              setToolsOpen(false)
              promptRef.current?.focus()
            }}
          />
        </Suspense>
      ) : null}
    </div>
  )
  const voiceBlock = (
    <div
      className="flex min-w-0 max-w-full flex-wrap items-center gap-0.5"
      data-testid="composer-voice-actions"
    >
      {showVoiceConversation && (
        <button
          type="button"
          className="grid !size-8 !min-w-8 place-items-center rounded-[var(--r-sm)] border border-transparent bg-[var(--surface-subtle)] text-[var(--text-muted)] transition-[background-color,color,border-color,box-shadow,transform] duration-200 hover:scale-105 hover:border-[var(--brand-blue)] hover:bg-[var(--brand-blue-soft)] hover:text-[var(--brand-blue-strong)]"
          title={t('chat:voiceMode.open')}
          aria-label={t('chat:voiceMode.open')}
          onClick={() => setVoiceModeOpen(true)}
        >
          <AudioLines size={17} />
        </button>
      )}
      {showVoiceInput && (
        <VoiceInputControl
          compact={!mobileLayout}
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
            const base = voiceDraftBaseRef.current ?? (promptRef.current?.value ?? value).trimEnd()
            voiceDraftBaseRef.current = null
            applyVoiceDraft(base ? `${base}\n${transcript}` : transcript)
            requestAnimationFrame(() => promptRef.current?.focus())
          }}
        />
      )}
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
      className={cn(
        'focus-composer-shell relative z-20 mx-auto flex shrink-0 flex-col gap-2 pt-2 pb-4',
        hasConversation
          ? 'w-[min(var(--chat-content-width),calc(100%_-_2_*_var(--chat-inline-padding)))]'
          : 'w-[min(600px,calc(100%_-_40px))] @max-[700px]:w-[calc(100%_-_24px)]',
      )}
      onSubmit={(event) => {
        if (
          !compactCommand &&
          !streaming &&
          (pendingRuntimeSelection || switchingModel || switchingThinking)
        ) {
          event.preventDefault()
          return
        }
        void submit(event)
      }}
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
      {commandError && (
        <p role="alert" className="px-1 text-sm text-destructive">
          {commandError}
        </p>
      )}
      <ComposerStatusPill
        compaction={compaction}
        streaming={streaming}
        statusLabel={composerStatusLabel}
      />
      <div
        className={
          mobileLayout ? 'contents' : 'min-w-0 rounded-[18px] bg-muted/40 dark:bg-[#202020]'
        }
      >
        {!mobileLayout && (
          <div className="flex min-w-0 items-center px-2.5 pt-1.5 pb-1">{workspaceBlock}</div>
        )}
        <div
          ref={composerAnchorRef}
          className="focus-composer relative flex min-w-0 flex-col gap-2 rounded-[18px] border border-border bg-card p-2.5 shadow-sm transition-[border-color,box-shadow] focus-within:border-ring/50 focus-within:shadow-md dark:bg-[#242424] [&_textarea]:w-full [&_textarea]:min-w-0 [&_textarea]:min-h-[40px] [&_textarea]:max-h-[220px] [&_textarea]:resize-none [&_textarea]:overflow-y-auto [&_textarea]:border-0 [&_textarea]:[outline:0]! [&_textarea]:bg-transparent [&_textarea]:px-1 [&_textarea]:py-1.5 [&_textarea]:text-[length:var(--app-message-font-size)] [&_textarea]:font-normal [&_textarea]:leading-relaxed [&_textarea]:text-foreground [&_textarea]:placeholder:text-muted-foreground"
        >
          {!toolsOpen && (
            <ComposerCommandMenu
              placement="top"
              sessionId={session.id}
              value={value}
              onChange={updateValue}
              inputRef={promptRef}
            />
          )}
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
          <div
            className={cn(
              'focus-composer-footer flex min-w-0 items-end gap-1',
              !mobileLayout && 'flex-wrap',
            )}
          >
            {toolsBlock}
            {!mobileLayout && voiceBlock}
            <ComposerSendButton
              streaming={streaming && !compactCommand}
              queueing={queueing}
              disabled={
                !compactCommand &&
                !streaming &&
                (Boolean(pendingRuntimeSelection) ||
                  switchingModel ||
                  switchingThinking ||
                  queueing ||
                  (!value.trim() && !selection.attachments.length && !invocation))
              }
              onAbort={onAbort}
            />
          </div>
        </div>
      </div>
      {mobileLayout && (
        <div
          className={cn(
            'composer-details flex min-h-9 min-w-0 flex-wrap items-center gap-2 px-2 py-1 text-xs text-muted-foreground',
            !detailsOpen && '!hidden',
          )}
        >
          {hasConversation && usageBlock}
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
            {mobileLayout && voiceBlock}
          </div>
        </div>
      )}
      {!hasConversation && !compactWelcome && !streaming && (
        <Suspense fallback={null}>
          <QuickPromptIdeas plansAvailable={plansAvailable} onPromptSelect={applyWelcomeChip} />
        </Suspense>
      )}
    </form>
  )
  return (
    <Panel
      ref={sessionViewport.ref}
      style={
        {
          '--chat-scrollbar-left': `${transcriptGutter.left}px`,
          '--chat-scrollbar-right': `${transcriptGutter.right}px`,
        } as CSSProperties
      }
      className={cn(
        '[--chat-inline-padding:24px] @max-[700px]:[--chat-inline-padding:16px] [--chat-column-padding:max(var(--chat-inline-padding),calc((100%_-_var(--chat-content-width))/2))]',
        `focus-session [--chat-content-width:768px] [.session-dock-panel_&]:overflow-hidden [.session-dock-panel_&]:min-h-0 [.session-dock-panel_&]:border-0 [.session-dock-panel_&]:rounded-[0] [.session-dock-panel_&]:bg-[var(--panel)] [.session-dock-panel_&]:p-0 [.session-dock-panel_&]:shadow-[none] [[data-theme='dark']_.session-dock-panel_&]:bg-[var(--main-surface-bg)] min-[651px]:[[data-density='compact']_.app-card:not(&)]:p-[10px] max-[650px]:min-h-[460px] max-[650px]:[.session-dock-panel_&]:min-h-0 relative flex h-full min-h-0 flex-col ${hasConversation ? 'has-conversation' : 'is-empty'}`,
        centeredWelcome &&
          '[&_[data-chat-region=messages]]:!flex-none [&_[data-chat-region=messages]]:mt-[max(48px,calc((var(--session-viewport-height,100dvh)-560px)/2))] [&_[data-chat-region=messages]]:h-[160px] [&_[data-chat-region=messages]]:!overflow-visible [&_.transcript]:!overflow-visible [&_.transcript]:!p-0 max-[650px]:[&_[data-chat-region=messages]]:mt-8 max-[650px]:[&_[data-chat-region=messages]]:h-[150px]',
      )}
      data-chat-message-style="bubble"
    >
      <SidebarStatusPortal enabled={!mobileLayout && shortcutEnabled}>
        <div
          className={cn(
            'flex min-w-0 justify-center py-1 [&_.context-usage-chip]:max-w-full [&_.context-usage-chip]:gap-1 [&_.context-usage-chip>i]:hidden',
            sidebarCollapsed &&
              '[&_.context-usage-chip]:!size-8 [&_.context-usage-chip]:!grid-cols-1 [&_.context-usage-chip]:justify-items-center [&_.context-usage-chip>span]:!hidden',
          )}
          data-usage-session-id={session.id}
        >
          {contextUsage?.contextWindow ? (
            <ContextUsageIndicator
              usage={contextUsage}
              sessionUsage={sessionUsage}
              model={model}
              availableModels={availableModels}
              onThresholdChange={onCompactionThresholdChange}
              compact={sidebarCollapsed}
            />
          ) : (
            <div className="flex min-w-0 flex-wrap items-center justify-center gap-1">
              <span
                className="inline-flex items-center gap-2 text-xs text-muted-foreground"
                title={t('chat:addMenu.usagePending')}
              >
                <Gauge size={16} />
                {!sidebarCollapsed && t('chat:addMenu.usagePending')}
              </span>
            </div>
          )}
        </div>
      </SidebarStatusPortal>
      <DesktopTitlebarPortal
        enabled={
          !mobileLayout && (titlebarTools ? titlebarToolsActive : Boolean(headerToolsTargetId))
        }
        targetId={titlebarTools ? 'window' : headerToolsTargetId}
      >
        {headerActions}
      </DesktopTitlebarPortal>
      <div
        className={cn(
          'flex h-full min-h-0 min-w-0 flex-1 flex-col',
          hasConversation ? 'overflow-hidden' : 'overflow-auto',
        )}
      >
        {mobileLayout && (
          <div className="shrink-0" data-chat-region="header">
            {headerBlock}
          </div>
        )}
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden" data-chat-region="messages">
          {transcriptBlock}
        </div>
        <div className="shrink-0" data-chat-region="composer">
          {composerBlock}
        </div>
      </div>
      <ChatResourcePicker
        open={resourcePickerOpen}
        initialCategory={resourceCategory}
        sessionId={session.id}
        onClose={() => setResourcePickerOpen(false)}
        onSelect={setInvocation}
        onCommandSelect={(commandInvocation) =>
          applyWelcomeChip(commandDraft(commandInvocation, value))
        }
      />
      <VoiceModeOverlay
        open={voiceModeOpen && showVoiceConversation}
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
