import { createPortal } from 'react-dom'
import { useContext } from 'react'
import { SessionRouteLeading } from './session-route-chrome'
import { TaskWorkspaceSlots } from './task-workspace-context'
// 聚焦会话视图：单会话沉浸式聊天页（大输入框 + 完整转录）。
// 拆分说明：props 类型在 focus-session-props，composer 状态与提交逻辑在
// use-focus-composer，输入区小组件（排队托盘/资源芯片/状态灯/按钮）在
// focus-session-composer-bits，状态文案在 focus-session-status；
// composer 主体与发送行为约定保留在本文件。
import { lazy, memo, Suspense, useEffect, useRef, useState, type DragEvent } from 'react'
import {
  AudioLines,
  Braces,
  Command,
  FolderOpen,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  Gauge,
  TerminalSquare,
  Pencil,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { DropdownMenuItem } from '@/components/ui/dropdown-menu'
import { OrbitSpinner } from '@/components/ui/spinner'
import { ThemeToggleButton } from '@/components/layout/ThemeToggleButton'
import { useUiStore } from '@/stores/ui-store'
import { AppCard as Panel } from '@/components/ui/app-primitives'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { workspaceName } from '@/lib/format'
import { cn } from '@/lib/utils'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { AttachmentPicker } from './AttachmentPicker'
import { isFileDrag } from './attachments'
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
import { ExecutionModeControl } from './GoalModeControl'
import { TaskWorkspaceHeader } from './TaskWorkspaceHeader'
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
  hasNewer,
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
  canClosePanel,
  onClosePanel,
  contextOpen,
  contextCompact,
  contextPanelId,
  onToggleContext,
  notify,
  onOpenModelSettings,
  onAssetConsumed,
  onLoadOlder,
  onLoadLatest,
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
  const slots = useContext(TaskWorkspaceSlots)
  // 会话页（路由模式）的返回入口：放在顶栏最左侧，不再单独占一行。
  const routeLeading = useContext(SessionRouteLeading)
  const { t, language } = useI18n()
  const cycleTheme = useUiStore((state) => state.cycleTheme)
  const theme = useUiStore((state) => state.theme)
  const shortcuts = useShortcutStore((state) => state.bindings)
  const COMMAND_PALETTE_SHORTCUT = useShortcutLabel('commandPalette')
  const mobileApp = useIsMobileApp()
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  // 手机顶栏只属于手机外壳；桌面壳窄窗口仍用工作区 / 标题的桌面写法，避免与自绘窗口按钮冲突。
  const phoneShell = mobileLayout && !window.pisperDesktop
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const goalsAvailable = runtimeFeatureAvailable(capabilities, 'goals')
  const teamAvailable = runtimeFeatureAvailable(capabilities, 'multiAgent')
  const plansAvailable = runtimeFeatureAvailable(capabilities, 'plans')
  const workflowsAvailable = runtimeFeatureAvailable(capabilities, 'workflows')
  const visualAvailable = runtimeFeatureAvailable(capabilities, 'visualGeneration')
  const { value, updateValue, selection, clearDraft, restoreDraft, captureSubmission } =
    useComposerDraft(session.id)
  const [resourcePickerOpen, setResourcePickerOpen] = useState(false)
  const [sessionTreeOpen, setSessionTreeOpen] = useState(false)
  const [voiceModeOpen, setVoiceModeOpen] = useState(false)
  const [toolsOpen, setToolsOpen] = useState(false)
  const [detailsOpen, setDetailsOpen] = useState(false)
  const [attachmentDropActive, setAttachmentDropActive] = useState(false)
  const [scrollRequest, setScrollRequest] = useState(0)
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
  const centeredWelcome = !hasConversation
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
    submitting,
    stopping,
    submitError,
    stop,
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
    captureSubmission,
    promptRef,
    onCompact,
    onGoalPause,
    onSend,
    onQueue,
    onAbort,
    setToolsOpen,
    requestTranscriptBottom,
  })
  const latestAgentMessage = messages.at(-1)?.role === 'agent' ? messages.at(-1) : null
  const currentError =
    submitError || error || (!streaming ? String(latestAgentMessage?.error || '') : '')
  const canRetryLastTurn =
    !hasNewer && !submitError && !streaming && messages.some((message) => message.role === 'user')

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
      event.preventDefault()
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
        className="resource-picker-trigger grid size-[30px] min-w-[30px] flex-none cursor-pointer place-items-center rounded-sm border-0 bg-transparent text-content-muted transition-colors hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-50 max-[650px]:size-11 max-[650px]:min-w-11 pointer-coarse:size-11 pointer-coarse:min-w-11"
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
        model={pendingRuntimeSelection?.model || model}
        models={availableModels}
        onModelChange={onModelChange}
        thinkingLevel={pendingRuntimeSelection?.thinkingLevel || thinkingLevel || 'medium'}
        levels={pendingRuntimeSelection?.model ? [] : availableThinkingLevels || []}
        status={thinkingStatus}
        message={thinkingMessage}
        onThinkingChange={onThinkingLevelChange}
        modelDisabled={switchingModel || switchingThinking}
        thinkingDisabled={switchingThinking || switchingModel}
        deferred={streaming}
        pendingModel={Boolean(pendingRuntimeSelection?.model)}
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
        className="command-palette-trigger relative grid size-[30px] min-w-[30px] flex-none cursor-pointer place-items-center rounded-sm border-0 bg-transparent text-content-muted transition-colors hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-50 max-[650px]:size-11 max-[650px]:min-w-11 pointer-coarse:size-11 pointer-coarse:min-w-11 [&_kbd]:sr-only"
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
  const availableComposerToolIds = COMPOSER_TOOL_IDS.filter((id) => composerTools[id] !== null)
  // Plan 是默认值时可退入「+」；切到 Goal/Team 或目标运行中必须常驻，否则手机上看不到生效的执行模式。
  const runModeEngaged = composerExecutionMode !== 'plan' || goal?.status === 'active'
  const toolbarAllocation = allocateComposerToolbar(
    toolbarLayout,
    availableComposerToolIds,
    // Respect the actual composer width; small layouts may move secondary settings into +.
    Math.max(toolbarCapacity, mobileLayout ? 5 : 8.5),
    // 按 40px 槽位计：窄屏审批芯片约 70px、执行模式图标 48px，模型按「名称 · 思考等级」的理想宽度计。
    mobileLayout
      ? { permission: 1.75, 'run-mode': 1.2, model: 4.4 }
      : { permission: 2.6, 'run-mode': 2, model: 3.9 },
    // 手机上审批芯片与模型名必须可读，空间不足时先把未启用的执行模式收进「+」。
    mobileLayout
      ? runModeEngaged
        ? ['permission', 'model', 'run-mode']
        : ['permission', 'model']
      : [],
  )
  const renderComposerTool = (id: ComposerToolId) => (
    <div
      className={cn(
        'composer-toolbar-slot flex min-w-0 flex-none items-center gap-2',
        // 模型药丸占据余下空间并靠右贴近发送键。常驻时槽位按余下宽度成为尺寸容器：
        // 放不下「名称 · 等级」先隐藏等级，再隐藏箭头，模型名始终保留（等级仍在弹层与无障碍名称里）。
        id === 'model' &&
          'ml-auto flex-1 justify-end [.composer-tool-tray_&]:ml-0 [.composer-tool-tray_&]:flex-none [.composer-tool-tray_&]:justify-start [&_.model-effort-pill]:max-w-full @max-[176px]/model-slot:[&_.model-effort-pill>span:nth-child(n+2)]:hidden @max-[124px]/model-slot:[&_.model-effort-pill>svg]:hidden',
        id === 'model' && toolbarAllocation.inline.includes(id) && '@container/model-slot',
        // 收纳区里整行是一个菜单项：统一悬停底色，图标不再各自带底，文字也可点击。
        '[.composer-tool-tray_&]:cursor-pointer [.composer-tool-tray_&]:rounded-sm [.composer-tool-tray_&]:pr-2 [.composer-tool-tray_&]:hover:bg-surface-hover [.composer-tool-tray_&_button]:!bg-transparent [.composer-tool-tray_&]:has-[button:disabled]:cursor-default [.composer-tool-tray_&]:has-[button:disabled]:opacity-50 [.composer-tool-tray_&]:has-[button:disabled]:hover:bg-transparent',
      )}
      data-composer-tool-id={id}
      key={id}
      onClick={(event) => {
        if (!event.currentTarget.closest('.composer-tool-tray')) return
        if ((event.target as Element).closest('button, a, input, select, [role="button"]')) return
        event.currentTarget.querySelector<HTMLButtonElement>('button:not(:disabled)')?.click()
      }}
    >
      {composerTools[id]}
      {!['model', 'permission', 'run-mode'].includes(id) && (
        <span className="hidden text-[13px] text-content [.composer-tool-tray_&]:block">
          {composerToolLabels[id]}
        </span>
      )}
    </div>
  )
  const workspaceBlock = (
    <button
      type="button"
      // 窄容器下与分隔符一起隐藏：只剩 32px 文件夹图标既不达触控尺寸又留下孤立的“/”；更多菜单仍可改工作目录。
      className="header-workspace inline-flex h-8 min-w-0 max-w-[150px] shrink items-center gap-1.5 rounded-md px-1.5 text-xs text-muted-foreground hover:bg-muted disabled:opacity-50 max-[650px]:h-11 pointer-coarse:h-11 @max-[520px]:hidden"
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
    >
      {(recallItem) => (
        <TaskWorkspaceHeader
          onRename={onRename}
          onWorkspace={onWorkspace}
          onClose={canClosePanel ? onClosePanel : undefined}
          sessionId={session.id}
          notify={notify}
          actions={
            <>
              {window.pisperDesktop?.terminalProfiles &&
                runtimeFeatureAvailable(capabilities, 'terminal') && (
                  <DropdownMenuItem
                    onSelect={() => window.dispatchEvent(new Event('pisper:toggle-terminal'))}
                  >
                    <TerminalSquare />
                    {t('navigation:pageHeader.toggleTerminal')}
                  </DropdownMenuItem>
                )}
              {recallItem}
              <DropdownMenuItem
                aria-controls={contextPanelId}
                onSelect={() => onToggleContext(!contextOpen)}
              >
                {contextOpen ? <PanelRightClose /> : <PanelRightOpen />}
                {contextOpen ? t('chat:sessionContext.close') : t('chat:sessionContext.open')}
              </DropdownMenuItem>
              <ThemeToggleButton menuItem theme={theme} onCycle={cycleTheme} />
            </>
          }
          data-window-drag-region
          data-context-aside={(contextOpen && !contextCompact) || undefined}
          className={cn(
            'workbench-chat-header relative z-4 h-14 shrink-0 items-center gap-2 border-0 bg-transparent pr-4 pl-[calc(16px+var(--workbench-leading-inset,0px))] py-1',
            // 手机单行顶栏：左侧让出与右侧两枚 44px 按钮等宽的位置（浮动菜单键落在其中），标题才能真正居中。
            phoneShell && 'gap-0 pr-1 pl-[92px]',
          )}
        >
          {phoneShell && routeLeading && (
            // 手机顶栏左侧本就为菜单键留出了位置，返回入口放在这里，标题仍然居中。
            <div className="absolute top-1/2 left-1 z-1 -translate-y-1/2">{routeLeading}</div>
          )}
          <div className="flex min-w-0 flex-1 items-center gap-1" data-window-drag-region>
            {!phoneShell && routeLeading && (
              <>
                {routeLeading}
                <span aria-hidden="true" className="mx-1 h-4 w-px flex-none bg-stroke" />
              </>
            )}
            {!phoneShell && (
              <>
                {workspaceBlock}
                <span className="text-muted-foreground/40 @max-[520px]:hidden" aria-hidden="true">
                  /
                </span>
              </>
            )}
            <button
              type="button"
              className={cn(
                'group inline-flex min-w-0 max-w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-[13px] font-medium hover:bg-muted max-[650px]:min-h-11 pointer-coarse:min-h-11',
                // 手机按设计稿两行居中：16/600 会话标题 + 12px 工作区与运行状态，整块仍是一个 44px 重命名目标。
                phoneShell && 'min-h-11 flex-1 flex-col justify-center gap-0 py-0 text-center',
              )}
              title={t('chat:focusSession.renameChat')}
              aria-label={t('chat:focusSession.renameChat')}
              onClick={onRename}
            >
              <span
                className={cn(
                  'truncate',
                  phoneShell && 'max-w-full text-lg leading-[1.3] font-semibold',
                )}
              >
                {session.name || t('navigation:pageHeader.newChat')}
              </span>
              {phoneShell ? (
                <span className="flex max-w-full items-center gap-1.5 text-xs leading-[1.3] font-normal text-content-muted">
                  <span className="truncate">{workspaceName(cwd, language)}</span>
                  {streaming && (
                    <>
                      <OrbitSpinner className="size-3" />
                      <span className="shrink-0">{t('chat:focusSession.running')}</span>
                    </>
                  )}
                </span>
              ) : (
                <Pencil
                  size={12}
                  aria-hidden="true"
                  className="shrink-0 opacity-0 group-hover:opacity-60 group-focus-visible:opacity-60"
                />
              )}
            </button>
          </div>
        </TaskWorkspaceHeader>
      )}
    </SessionTreeControl>
  )
  const transcriptBlock = (
    <FocusTranscript
      key="transcript"
      layoutMeasurementKey="fixed"
      sessionId={session.id}
      messages={messages}
      transcriptLoadState={transcriptLoadState}
      messageStart={messageStart}
      hasNewer={hasNewer}
      onLoadLatest={onLoadLatest}
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
      className="focus-composer-quick-actions flex min-w-0 flex-1 items-center gap-1"
    >
      <button
        ref={toolTrayAnchorRef}
        type="button"
        className={`composer-tools-trigger grid size-[30px] min-w-[30px] flex-none cursor-pointer place-items-center rounded-sm border border-stroke bg-transparent text-content-secondary transition-colors hover:bg-surface-hover hover:text-content max-[650px]:size-11 max-[650px]:min-w-11 max-[650px]:rounded-md max-[650px]:border-transparent pointer-coarse:size-11 pointer-coarse:min-w-11 ${toolsOpen ? 'active bg-surface-hover text-content' : ''}`}
        title={quickActionsLabel}
        aria-label={quickActionsLabel}
        aria-expanded={toolsOpen}
        aria-controls={toolTrayId}
        onClick={() => setToolsOpen((open) => !open)}
      >
        {toolsOpen ? <X size={17} /> : <Plus size={18} />}
      </button>
      <div className="focus-composer-visible-tools flex min-w-0 flex-1 flex-wrap items-center gap-1">
        {toolbarAllocation.inline.map(renderComposerTool)}
      </div>
      <ComposerToolTray
        placement="top"
        open={toolsOpen}
        label={t('chat:focusSession.quickActions')}
        trayId={toolTrayId}
        anchorRef={toolTrayAnchorRef}
        menuRef={toolTrayMenuRef}
      >
        {toolbarAllocation.overflow.map(renderComposerTool)}
        <button
          type="button"
          className="flex h-[30px] w-full cursor-pointer items-center gap-2 rounded-sm pr-2 text-[13px] text-content hover:bg-surface-hover max-[650px]:h-11 pointer-coarse:h-11"
          aria-expanded={detailsOpen}
          onClick={() => {
            setDetailsOpen((open) => !open)
            setToolsOpen(false)
          }}
        >
          <span className="grid size-[30px] shrink-0 place-items-center text-content-muted max-[650px]:size-11 pointer-coarse:size-11">
            <Gauge size={16} />
          </span>
          {detailsOpen ? t('chat:focusSession.hideDetails') : t('chat:focusSession.details')}
        </button>
        <div className="w-full border-t border-stroke-soft pt-1">
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
  const showAttachmentDrop = (event: DragEvent<HTMLDivElement>) => {
    if (!isFileDrag(event.dataTransfer)) return false
    event.preventDefault()
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy'
    setAttachmentDropActive(true)
    return true
  }
  const hideAttachmentDrop = (event: DragEvent<HTMLDivElement>) => {
    const next = event.relatedTarget
    if (next instanceof Node && event.currentTarget.contains(next)) return
    setAttachmentDropActive(false)
  }
  const composerBlock = (
    <form
      key="composer"
      className={cn(
        'focus-composer-shell relative z-20 mx-auto flex w-[min(768px,calc(100%_-_48px))] shrink-0 flex-col gap-2 pt-2 pb-3 @max-[700px]:w-[calc(100%_-_24px)] max-[650px]:pb-2.5',
        // 有对话时在输入区上方叠 36px 渐隐，让转录滚到底部时柔和收尾而不是硬切。
        hasConversation &&
          'before:pointer-events-none before:absolute before:inset-x-0 before:-top-9 before:h-9 before:bg-linear-to-b before:from-transparent before:to-panel',
      )}
      onSubmit={(event) => {
        if (!streaming && (pendingRuntimeSelection || switchingModel || switchingThinking)) {
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
      {/* 输入卡片：控件级描边（≥3:1）+ 输入时焦点描边与 3px 焦点环；运行条集成在卡片顶部。 */}
      <div
        className={cn(
          'focus-composer relative flex min-w-0 flex-col rounded-[var(--composer-radius,22px)] [corner-shape:var(--composer-corner-shape,round)] border border-control bg-panel shadow-composer transition-[border-color,box-shadow] duration-(--d-fast) ease-(--ease) has-[textarea:focus]:border-focus has-[textarea:focus]:ring-3 has-[textarea:focus]:ring-focus-ring max-[650px]:rounded-2xl [&>.attachment-tray]:px-3 [&>.attachment-tray]:pt-3 [&_textarea]:w-full [&_textarea]:min-w-0 [&_textarea]:min-h-11 [&_textarea]:max-h-[220px] [&_textarea]:resize-none [&_textarea]:overflow-y-auto [&_textarea]:border-0 [&_textarea]:[outline:0]! [&_textarea]:bg-transparent [&_textarea]:px-4 [&_textarea]:pt-3 [&_textarea]:pb-1 [&_textarea]:text-[length:calc(var(--app-message-font-size)*.9375)] max-[650px]:[&_textarea]:px-3.5 max-[650px]:[&_textarea]:text-[length:var(--app-message-font-size)] [&_textarea]:[font-family:var(--font-content-family)] [&_textarea]:font-[400] [&_textarea]:leading-5 [&_textarea]:text-content [&_textarea]:placeholder:text-content-muted',
          attachmentDropActive && 'border-focus ring-3 ring-focus-ring',
        )}
        onDragEnter={showAttachmentDrop}
        onDragOver={showAttachmentDrop}
        onDragLeave={hideAttachmentDrop}
        onDrop={(event) => {
          setAttachmentDropActive(false)
          selection.dropFiles(event)
        }}
      >
        {attachmentDropActive && (
          <div
            role="status"
            className="pointer-events-none absolute inset-0 z-10 grid place-items-center rounded-[inherit] border border-dashed border-focus bg-panel px-4 text-center text-sm text-content"
          >
            {t('chat:focusSession.dropAttachment')}
          </div>
        )}
        <ComposerStatusPill
          compaction={compaction}
          streaming={streaming}
          statusLabel={composerStatusLabel}
          startedAt={runStartedAt}
          activity={currentActivity}
          stopping={stopping}
          onStop={() => void stop()}
        />
        <AttachmentTray
          attachments={selection.attachments}
          preparing={selection.preparingAttachments}
          onRemove={selection.removeAttachment}
        />
        <ComposerCommandMenu
          placement="top"
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
            // 运行条提示「Esc 停止」：输入框内按 Esc 中断当前运行；命令菜单等已处理的 Esc 不再重复响应，
            // 工具托盘打开时 Esc 只收起托盘（托盘监听在 document 上，晚于这里触发）。
            if (
              streaming &&
              !composing &&
              !toolsOpen &&
              event.key === 'Escape' &&
              !event.defaultPrevented
            ) {
              event.preventDefault()
              void stop()
            }
          }}
          data-mobile-composer-input={mobileLayout || undefined}
          enterKeyHint={mobileLayout && shortcuts.sendMessage === 'Enter' ? 'send' : 'enter'}
          placeholder={composerPlaceholder}
        />
        <div className="focus-composer-footer @container/composer-footer flex min-w-0 items-center gap-1 px-2 pt-1 pb-2 max-[650px]:px-1.5 max-[650px]:pb-1.5">
          {toolsBlock}
          <ComposerSendButton
            streaming={streaming}
            queueing={queueing}
            submitting={submitting}
            stopping={stopping}
            preparing={selection.preparingAttachments.length > 0}
            hasDraft={Boolean(value.trim() || selection.attachments.length || invocation)}
            canQueue={Boolean(onQueue)}
            blockedReason={
              !streaming && (pendingRuntimeSelection || switchingModel || switchingThinking)
                ? t('chat:focusSession.waitingForModel')
                : undefined
            }
            disabled={
              !streaming &&
              (Boolean(pendingRuntimeSelection) ||
                switchingModel ||
                switchingThinking ||
                queueing ||
                (!value.trim() && !selection.attachments.length && !invocation))
            }
          />
        </div>
      </div>
      <div
        className={cn(
          'composer-details flex min-h-9 min-w-0 flex-wrap items-center gap-2 px-2 py-1 text-xs text-content-muted',
          !detailsOpen && '!hidden',
        )}
      >
        {hasConversation && usageBlock}
        <button
          type="button"
          className="grid size-7 cursor-pointer place-items-center rounded-sm hover:bg-surface-hover hover:text-content max-[650px]:size-11 pointer-coarse:size-11"
          aria-label={t('chat:focusSession.hideDetails')}
          onClick={() => setDetailsOpen(false)}
        >
          <X size={14} />
        </button>
        <div className="ml-auto flex items-center gap-1">
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
              className="grid size-8 min-w-8 cursor-pointer place-items-center rounded-sm text-content-muted transition-colors hover:bg-surface-hover hover:text-content max-[650px]:size-11 pointer-coarse:size-11"
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
      {!hasConversation && !streaming && (
        <Suspense fallback={null}>
          <QuickPromptIdeas plansAvailable={plansAvailable} onPromptSelect={applyWelcomeChip} />
        </Suspense>
      )}
    </form>
  )
  return (
    <Panel
      className={cn(
        `focus-session [--chat-content-width:768px] [.session-dock-panel_&]:overflow-hidden [.session-dock-panel_&]:min-h-0 [.session-dock-panel_&]:border-0 [.session-dock-panel_&]:rounded-[0] [.session-dock-panel_&]:bg-[var(--panel)] [.session-dock-panel_&]:p-0 [.session-dock-panel_&]:shadow-[none] [[data-theme='dark']_.session-dock-panel_&]:bg-[var(--main-surface-bg)] min-[651px]:[[data-density='compact']_.app-card:not(&)]:p-[10px] max-[650px]:min-h-[460px] max-[650px]:[.session-dock-panel_&]:min-h-0 relative flex h-full min-h-0 flex-col ${hasConversation ? 'has-conversation' : 'is-empty'}`,
        centeredWelcome &&
          '[&_[data-chat-region=messages]]:!flex-none [&_[data-chat-region=messages]]:mt-[max(48px,calc((100dvh-560px)/2))] [&_[data-chat-region=messages]]:h-[96px] [&_[data-chat-region=messages]]:!overflow-visible [&_.transcript]:!overflow-visible [&_.transcript]:!p-0 max-[650px]:[&_[data-chat-region=messages]]:mt-8 max-[650px]:[&_[data-chat-region=messages]]:h-[96px]',
      )}
      data-chat-message-style="bubble"
    >
      <div
        className={cn(
          'flex h-full min-h-0 min-w-0 flex-1 flex-col',
          hasConversation ? 'overflow-hidden' : 'overflow-auto',
        )}
      >
        <div className="shrink-0" data-chat-region="header">
          {slots?.headerHost
            ? createPortal(headerBlock, slots.headerHost)
            : !slots
              ? headerBlock
              : null}
        </div>
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden" data-chat-region="messages">
          {transcriptBlock}
        </div>
        <div className="shrink-0" data-chat-region="composer">
          {composerBlock}
        </div>
      </div>
      {slots?.sourceActionHost &&
        createPortal(<AttachmentPicker cwd={cwd} selection={selection} />, slots.sourceActionHost)}
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
        sendPrompt={async (...args) => {
          await onSend(...args)
        }}
        onAbort={onAbort}
        onClose={() => setVoiceModeOpen(false)}
      />
    </Panel>
  )
})
