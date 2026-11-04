import { createPortal } from 'react-dom'
import { useContext, useMemo } from 'react'
import { SessionRouteLeading } from './session-route-chrome'
import {
  SessionInspectorContext,
  SessionPageChromeContext,
  TranscriptMetaContext,
} from './session-workspace-context'
import { SessionPageHeader } from './SessionPageHeader'
import { SessionPhoneHeader } from './SessionPhoneHeader'
import { TaskWorkspaceSlots } from './task-workspace-context'
// 聚焦会话视图：单会话沉浸式聊天页（大输入框 + 完整转录）。
// 拆分说明：props 类型在 focus-session-props，composer 状态与提交逻辑在
// use-focus-composer，输入区小组件（托盘/资源芯片/发送键）在 focus-session-composer-bits，
// 盾牌芯片在 ComposerModeChip，模型芯片与用量在 FocusRuntimeControls；
// composer 主体与发送行为约定保留在本文件。
import { memo, useCallback, useEffect, useRef, useState, type DragEvent } from 'react'
import { Braces, CornerDownRight, ImagePlus, Mic, Paperclip, Plus, Shield } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { AppCard as Panel } from '@/components/ui/app-primitives'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { compactModelName } from '@/lib/model-display'
import { cn } from '@/lib/utils'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { AttachmentPicker } from './AttachmentPicker'
import { useAttachmentChooser } from './use-attachment-chooser'
import { isFileDrag } from './attachments'
import { AttachmentTray } from './AttachmentTray'
import { ChatResourcePicker } from './ChatResourcePicker'
import { ComposerCommandMenu } from './ComposerCommandMenu'
import { commandDraft, NEW_TASK_COMPOSER_KEY, useComposerDraft } from './composer-drafts'
import { NewTaskDraftHeader, NewTaskGreeting } from './NewTaskDraftView'
import { consumeNewTaskFocus, NEW_TASK_FOCUS_EVENT } from './events'
import { ChatRequestNotice } from './ChatRequestNotice'
import { ComposerModeChip } from './ComposerModeChip'
import { nextComposerRunMode } from './composer-run-mode'
import {
  ContextUsageSection,
  ModelThinkingControl,
  thinkingLevelLabel,
} from './FocusRuntimeControls'
import { runtimeSettingCommands, sideQuestionText, type BuiltinCommand } from './builtin-commands'
import { SideQuestionPanel } from './SideQuestionPanel'
import { useSideQuestion } from './useSideQuestion'
import { chatApi } from './chat-api'
import { FocusTranscript } from './FocusTranscript'
import { SessionTreeControl } from './SessionTreeControl'
import { SessionWorkflowRuns } from './SessionWorkflowRuns'
import { ToolApproval } from './ToolApproval'
import { QuestionCard } from './QuestionCard'
import { parsePendingQuestions } from './session-questions'
import { openVisualPromptEntry } from './visual-prompt-entry'
import { VoiceInputControl } from './VoiceInputControl'
import { VoiceModeOverlay } from './VoiceModeOverlay'
import {
  COMPOSER_ICON_KEY_CLASSES,
  COMPOSER_PHONE_GLYPH_CLASSES,
  ComposerResourceChip,
  ComposerSendButton,
  ComposerTray,
} from '@/features/chat/focus-session-composer-bits'
import type { FocusSessionProps } from '@/features/chat/focus-session-props'
import { useFocusComposer } from '@/features/chat/use-focus-composer'
import { useShortcutStore } from '@/stores/shortcut-store'
import {
  formatShortcut,
  isEditableTarget,
  matchesShortcut,
  shortcutEventBlocked,
  useShortcutLabel,
} from '@/lib/shortcuts'
import { isPendingQueuedMessage, lastSettledMessageIndex } from '@/lib/session-state'

export type { FocusSessionProps }

// Esc 先交给打开的菜单、弹层、对话框（与 BackNavigation 同一判定），它们都关着才轮到“停止运行”。
// 弹层包括 Radix 的 Popover（盾牌芯片、模型芯片）与锚定弹层（/ 命令菜单）。
const ESCAPE_LAYER_SELECTOR = [
  '[role="dialog"]',
  '[role="alertdialog"]',
  '[role="menu"]',
  '[role="listbox"]',
  '.anchored-popup-menu',
  '.voice-input-popup',
  '.git-diff-dialog-backdrop',
  "[data-slot='popover-content']",
  "[data-slot='dropdown-menu-content']",
  "[data-slot='select-content']",
  "[data-slot='sheet-content']",
].join(', ')
function escapeLayerOpen() {
  return [...document.querySelectorAll<HTMLElement>(ESCAPE_LAYER_SELECTOR)].some(
    (element) =>
      element.dataset.state !== 'closed' &&
      element.getAttribute('aria-hidden') !== 'true' &&
      element.getClientRects().length > 0,
  )
}

export const FocusSession = memo(function FocusSession({
  session,
  draft,
  emptyState,
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
  questions,
  error,
  pendingAsset,
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
  onAnswerQuestion,
  onWorkspace,
  onRename,
  onBranchFromHere,
  onCreateChildSession,
  onRetryLastTurn,
  onTreeNavigated,
  onSend,
  onQueue,
  onWithdrawQueuedInput,
  onSendQueuedInputNow,
  onAbort,
}: FocusSessionProps) {
  const slots = useContext(TaskWorkspaceSlots)
  // 会话页（路由模式）的返回入口：放在顶栏最左侧，不再单独占一行。
  const routeLeading = useContext(SessionRouteLeading)
  // 会话页：桌面布局的页头换成 44px 会话页头，返回入口在泳道条上；
  // 手机外壳（pageChrome.phone）换成 56px 顶栏，返回入口在顶栏左侧。
  const pageChrome = useContext(SessionPageChromeContext)
  const phonePage = Boolean(pageChrome?.phone)
  const { t } = useI18n()
  const shortcuts = useShortcutStore((state) => state.bindings)
  const mobileApp = useIsMobileApp()
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const goalsAvailable = runtimeFeatureAvailable(capabilities, 'goals')
  const teamAvailable = runtimeFeatureAvailable(capabilities, 'multiAgent')
  const workflowsAvailable = runtimeFeatureAvailable(capabilities, 'workflows')
  const visualAvailable = runtimeFeatureAvailable(capabilities, 'visualGeneration')
  // 插队（立即发送）：Runtime 支持时打断当前这一步；旧 Runtime 退回排队（下一步送达），排队项也没有“立即发送”。
  const interruptAvailable = runtimeFeatureAvailable(capabilities, 'inputInterrupt')
  const sendNowBehavior = interruptAvailable ? 'interrupt' : 'steer'
  const { value, updateValue, selection, clearDraft, restoreDraft, captureSubmission } =
    useComposerDraft(draft ? NEW_TASK_COMPOSER_KEY : session.id)
  const [resourcePickerOpen, setResourcePickerOpen] = useState(false)
  // 手机：审批与执行方式的底部表单从 + 菜单打开（工具行上的芯片默认收起）。
  const [modeSheetOpen, setModeSheetOpen] = useState(false)
  // + 菜单里打开别的弹层：等菜单关闭、焦点回到 + 键之后再开，关掉表单时焦点才回得到 + 键。
  const afterPlusMenuCloseRef = useRef<(() => void) | null>(null)
  const plusTriggerRef = useRef<HTMLButtonElement>(null)
  const [sessionTreeOpen, setSessionTreeOpen] = useState(false)
  const [voiceModeOpen, setVoiceModeOpen] = useState(false)
  // ⇧Tab 切换执行方式后的读屏播报（芯片文字同时改变）。
  const [modeAnnouncement, setModeAnnouncement] = useState('')
  const [attachmentDropActive, setAttachmentDropActive] = useState(false)
  const [scrollRequest, setScrollRequest] = useState(0)
  const addSelectedAttachments = selection.addAttachments
  const promptRef = useRef<HTMLTextAreaElement>(null)
  const panelRootRef = useRef<HTMLDivElement>(null)
  // 会话页运行中（参考 Claude）：↵ 排队（steer，等当前这一步走完就送达）、⌥↵ 立即发送（interrupt，
  // 打断当前这一步马上送达）。提交前记下这次的方式，表单提交时读取后复位。
  const runningBehaviorRef = useRef<'steer' | 'interrupt'>('steer')
  // 立即发送键是独立绑定（默认 ⌥↵，可在设置里改绑或禁用），不再由发送键派生；设置项沿用 steerMessage 的键名。
  const steerShortcut = shortcuts.steerMessage
  // 触屏没有 ⌥：手机会话页运行中长按发送键或在“+”菜单里选“立即发送”，都走同一条提交路径。
  const submitSteer = () => {
    runningBehaviorRef.current = sendNowBehavior
    promptRef.current?.form?.requestSubmit()
  }
  const SEND_SHORTCUT = useShortcutLabel('sendMessage')
  const VOICE_SHORTCUT = useShortcutLabel('voiceInput')
  // 语音实时转写与草稿的合并基准：录音开始时记住原文本，部分转写反复覆盖增量段。
  const voiceDraftBaseRef = useRef<string | null>(null)
  // “+”菜单里的“语音输入”经它触发与 F8 相同的开始 / 结束（VoiceInputControl 的 menu 形态）。
  const voiceControlRef = useRef<(() => void) | null>(null)
  useEffect(() => {
    if (!shortcutEnabled) return
    const focusComposer = (event: KeyboardEvent) => {
      if (!shortcutEventBlocked(event) && matchesShortcut(event, shortcuts.focusComposer)) {
        event.preventDefault()
        promptRef.current?.focus()
      }
    }
    window.addEventListener('keydown', focusComposer)
    return () => window.removeEventListener('keydown', focusComposer)
  }, [shortcutEnabled, shortcuts.focusComposer])
  // 新任务草稿：桌面进入即可输入；手机只在主动发起新任务（⌘N、✎、抽屉“新任务”）时聚焦，
  // 落地主页的草稿不聚焦，免得一打开就弹出软键盘。已在草稿上再点“新任务”时把光标放回输入框末尾。
  const isDraft = Boolean(draft)
  useEffect(() => {
    if (!isDraft) return
    const focusEnd = () => {
      const element = promptRef.current
      if (!element) return
      element.focus()
      element.setSelectionRange(element.value.length, element.value.length)
      // Chromium 系（含 WebView2、Android WebView）设置选区不会滚动：长草稿要手动滚到底才看得到光标。
      element.scrollTop = element.scrollHeight
      consumeNewTaskFocus()
    }
    if (consumeNewTaskFocus() || !mobileLayout) focusEnd()
    window.addEventListener(NEW_TASK_FOCUS_EVENT, focusEnd)
    return () => window.removeEventListener(NEW_TASK_FOCUS_EVENT, focusEnd)
  }, [isDraft, mobileLayout])
  // 输入法组词跟踪：Mac WebKit 的确认 Enter 在 compositionend 后才派发，需自行跟踪并延迟复位。
  const imeComposingRef = useRef(false)
  const hasConversation = transcriptLoadState !== 'ready' || messages.length > 0
  // 占位文字（规格 v2 第 4.3 节）：空闲“回复代理，或用 / 调用命令”；运行中把排队 / 插话的按法
  // 写进占位文字（原来工具条上的“↵ 排队 · ⌥↵ 插话”提示退役）。触屏没有 ⌥：点发送排队，长按插话。
  const composerPlaceholder = streaming
    ? mobileLayout
      ? t('board:composer.placeholderRunningTouch')
      : steerShortcut
        ? t('board:composer.placeholderRunning', {
            queue: SEND_SHORTCUT,
            steer: formatShortcut(steerShortcut),
          })
        : t('board:composer.placeholderRunningQueueOnly', { queue: SEND_SHORTCUT })
    : draft
      ? t('chat:newTask.placeholder')
      : t('board:composer.placeholder')

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
    requestTranscriptBottom,
  })
  // 会话页 Esc（规格 2.8）：有打开的菜单 / 浮层时先交给它们；都关着且正在运行时停止运行。
  // Esc 永远不用于返回。只认会话对话列（页头、转录、输入框）或页面本身的焦点，
  // 检查器里的终端、侧聊输入框等有自己的 Esc 语义，不在这里截走。
  const stopRef = useRef(stop)
  stopRef.current = stop
  useEffect(() => {
    if (!pageChrome || !shortcutEnabled || !streaming) return undefined
    const layered = new WeakSet<Event>()
    // 捕获阶段记下“按下时有没有浮层开着”：浮层在冒泡前就可能已经关掉，不能等冒泡时再看。
    const capture = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && escapeLayerOpen()) layered.add(event)
    }
    const bubble = (event: KeyboardEvent) => {
      // 原生返回键（Android、手机 App 平板）由 BackNavigation 合成一个 Esc 交给浮层：
      // 它不是用户按下的 Esc，不能被当成“停止运行”，没有浮层时要落到路由返回。
      if (
        event.key !== 'Escape' ||
        !event.isTrusted ||
        event.defaultPrevented ||
        event.isComposing ||
        event.keyCode === 229 ||
        layered.has(event)
      )
        return
      const target = event.target
      const root = panelRootRef.current
      const inConversation =
        target === document.body ||
        target === document.documentElement ||
        (target instanceof Element &&
          Boolean(target.closest('[data-session-conversation]')) &&
          !target.closest('.terminal-panel, .xterm'))
      if (!root || !inConversation) return
      // 输入框之外的可编辑控件（例如改名框）按自己的语义处理 Esc。
      if (isEditableTarget(target) && target !== promptRef.current) return
      event.preventDefault()
      void stopRef.current()
    }
    window.addEventListener('keydown', capture, true)
    window.addEventListener('keydown', bubble)
    return () => {
      window.removeEventListener('keydown', capture, true)
      window.removeEventListener('keydown', bubble)
    }
  }, [pageChrome, shortcutEnabled, streaming])
  // 输入框工具条与会话页头“⋯ → 目标模式”共用：切换前先暂停与新模式不一致的进行中目标。
  const changeExecutionMode = (nextMode: typeof composerExecutionMode) => {
    if (goal?.status === 'active' && (nextMode === 'plan' || nextMode !== goal.mode))
      void requestGoalPause().catch(() => {})
    setComposerExecutionMode(nextMode)
    void onRunModeChange(nextMode)
  }
  // 输入框的内置斜杠命令（与 TUI 对齐）：都走输入框芯片与页头已有的同一组切换函数。
  const inspector = useContext(SessionInspectorContext)
  const onNewTask = pageChrome?.onNewTask || pageChrome?.phone?.onNewTask
  // 新任务草稿还没有会话：按所选项目目录列出同一套 Prompt / Skill 命令。
  const draftCwd = draft ? draft.cwd : null
  const loadSessionCommands = useCallback(
    () =>
      draftCwd !== null
        ? chatApi.getWorkspaceCommands(draftCwd)
        : chatApi.getSessionCommands(session.id),
    [draftCwd, session.id],
  )
  const sideQuestion = useSideQuestion(session.id)
  // 每次渲染重建：命令直接调用当前的切换函数，不缓存旧闭包；数组很小，菜单只在打开时匹配。
  const builtinCommands: BuiltinCommand[] = [
    ...(onNewTask && !draft
      ? [{ invocation: '/new', description: t('chat:commands.builtin.new'), run: onNewTask }]
      : []),
    ...runtimeSettingCommands({
      t,
      models: availableModels,
      model: pendingRuntimeSelection?.model || model,
      onModel: (key) => onModelChange(key),
      thinkingLevels: pendingRuntimeSelection?.model ? [] : availableThinkingLevels || [],
      thinkingLevel: pendingRuntimeSelection?.thinkingLevel || thinkingLevel || '',
      thinkingLabel: (level) => thinkingLevelLabel(t, level),
      onThinking: (level) => onThinkingLevelChange(level),
      executionModes: ['approval-required', 'workspace-write', 'full-access'],
      executionMode,
      executionLabel: (mode) =>
        mode === 'full-access'
          ? t('chat:focusSession.fullAccess')
          : mode === 'workspace-write'
            ? t('chat:focusSession.workspaceWrite')
            : t('chat:focusSession.approvalRequired'),
      onExecutionMode: (mode) => onExecutionModeChange(mode),
      // 运行中输入框的方式芯片是禁用的，命令同样不提供。
      runModes: streaming
        ? []
        : [
            'plan',
            ...(goalsAvailable ? ['goal'] : []),
            ...(goalsAvailable && teamAvailable ? ['team'] : []),
          ],
      runMode: composerExecutionMode,
      runModeLabel: (mode) =>
        mode === 'team'
          ? t('chat:focusSession.teamMode')
          : mode === 'goal'
            ? t('chat:focusSession.goalMode')
            : t('chat:focusSession.planMode'),
      onRunMode: (mode) => changeExecutionMode(mode as typeof composerExecutionMode),
    }),
    // 旁问是分组形态：选中后补全成“/btw ”，写完问题再发送。草稿还没有对话，不提供。
    ...(draft ? [] : [{ invocation: '/btw', description: t('chat:commands.builtin.btw') }]),
    ...(onCompact
      ? [
          {
            invocation: '/compact',
            description: t('chat:commands.builtin.compact'),
            run: () => compactContext(),
          },
        ]
      : []),
    ...(inspector && !draft
      ? [
          {
            invocation: '/changes',
            description: t('chat:commands.builtin.changes'),
            run: () => inspector.show('changes'),
          },
        ]
      : []),
  ]
  // 会话页的代理信息行（规格 v2 第 4.2 节）：最新一条回复写当前模型（显示名）。
  // 审批模式已在输入框的盾牌芯片上，不再在信息行重复；思考用时等 Runtime 有数据（A9）再加。
  const currentModel = pendingRuntimeSelection?.model || model || ''
  const modelLabel = compactModelName(
    availableModels.find((item) => item.key === currentModel)?.label ||
      currentModel.split('/').at(-1) ||
      '',
    availableModels.map((item) => item.label),
  )
  // Bot 的长期会话：模型、权限在 Bot 的设置页里定，输入框不放这两个芯片；回复的信息行换成 Bot 的头像与名字。
  const botId = session?.botId
  const transcriptMeta = useMemo(
    () => (pageChrome ? { model: modelLabel, ...(botId ? { botId } : {}) } : null),
    [botId, modelLabel, pageChrome],
  )
  // 还没送达的排队消息：桌面在输入框上方的排队列表里（可立即发送 / 编辑 / 取消），对话里不重复画；
  // 手机没有排队列表，它们就是对话末尾标着“排队中”的气泡。
  const transcriptMessages = useMemo(
    () => (phonePage ? messages : messages.filter((message) => !isPendingQueuedMessage(message))),
    [messages, phonePage],
  )
  // 标题菜单“压缩上下文”旁的用量（规格 v2 第 2.5 节）：与模型芯片弹层同一来源，拿不到时不写。
  const contextPercent = Number(contextUsage?.percent)
  const runMetricsContextPercent =
    contextUsage?.percent != null && Number.isFinite(contextPercent) ? contextPercent : null
  const latestSettled = messages[lastSettledMessageIndex(messages)]
  const latestAgentMessage = latestSettled?.role === 'agent' ? latestSettled : null
  const currentError =
    submitError || error || (!streaming ? String(latestAgentMessage?.error || '') : '')
  const canRetryLastTurn =
    !hasNewer && !submitError && !streaming && messages.some((message) => message.role === 'user')

  useEffect(() => {
    setResourcePickerOpen(false)
    setSessionTreeOpen(false)
  }, [session.id])
  useEffect(() => {
    if (!pendingAsset) return
    addSelectedAttachments([pendingAsset])
    onAssetConsumed?.()
  }, [pendingAsset, onAssetConsumed, addSelectedAttachments])
  // 托盘里排队消息的“编辑”：撤回后合并回输入框（与已有草稿合并，不覆盖），焦点回到输入框。
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

  // 托盘里的“取消”：只撤回这条排队消息，不放回输入框。
  const cancelQueuedInput = async (inputId: string) => {
    await onWithdrawQueuedInput?.(inputId)
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
  const attachmentChooser = useAttachmentChooser({ cwd, selection })
  const voiceInput = (
    <VoiceInputControl
      sessionId={session.id}
      shortcutEnabled={shortcutEnabled}
      // 手机照参考稿把麦克风键放在发送键旁边；桌面入口在 + 菜单里（F8 照常）。
      variant={phonePage ? 'button' : 'menu'}
      controlRef={voiceControlRef}
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
  )
  const plusLabel = t('board:composer.plusMenu')
  // 输入框“+”菜单（规格 v2 第 2.5 / 4.3 节）：附件、技能与工具、生成图片、语音输入（F8）；
  // 手机上语音键在工具行，这里换成“审批与执行方式”；手机运行中另有“插话发送”（触屏没有 ⌥↵）。原来可配置的工具条与“+”托盘退役，工具行固定为
  // [+] [盾牌芯片] … [模型芯片] [发送]。
  const plusMenu = (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className={cn(
            COMPOSER_ICON_KEY_CLASSES,
            // 手机：浅灰底圆形，和盾牌、模型胶囊同一种无边框填充样式。
            'composer-plus-trigger aria-expanded:bg-surface-hover aria-expanded:text-content max-[650px]:rounded-full max-[650px]:bg-surface-muted max-[650px]:dark:bg-surface-active max-[650px]:text-content',
          )}
          ref={plusTriggerRef}
          title={plusLabel}
          aria-label={plusLabel}
          data-composer-plus
        >
          <Plus size={18} className={COMPOSER_PHONE_GLYPH_CLASSES} aria-hidden="true" />
          {selection.attachments.length > 0 && (
            <span className="absolute -top-0.5 -right-0.5 grid h-4 min-w-4 place-items-center rounded-full border-2 border-panel bg-star px-0.5 text-[10px] leading-none font-medium text-on-accent tabular-nums">
              {selection.attachments.length}
            </span>
          )}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        side="top"
        className="min-w-56 p-1 [&_[role=menuitem]]:min-h-9 max-[650px]:[&_[role=menuitem]]:min-h-11 pointer-coarse:[&_[role=menuitem]]:min-h-11"
        data-composer-plus-menu
        onCloseAutoFocus={() => {
          const next = afterPlusMenuCloseRef.current
          afterPlusMenuCloseRef.current = null
          if (next) window.setTimeout(next, 0)
        }}
      >
        {phonePage && streaming && (
          <DropdownMenuItem
            data-composer-steer
            disabled={!value.trim() && !selection.attachments.length && !invocation}
            onSelect={submitSteer}
          >
            <CornerDownRight aria-hidden="true" />
            <span className="grid min-w-0">
              <span>{t('board:composer.steerNow')}</span>
              <span className="truncate text-xs text-content-muted">
                {t('board:composer.steerHint')}
              </span>
            </span>
          </DropdownMenuItem>
        )}
        <DropdownMenuItem
          data-composer-plus-item="attachment"
          onSelect={() => void attachmentChooser.choose()}
        >
          <Paperclip aria-hidden="true" />
          {t('chat:focusSession.addAttachment')}
        </DropdownMenuItem>
        <DropdownMenuItem
          data-composer-plus-item="resource"
          onSelect={() => setResourcePickerOpen(true)}
        >
          <Braces aria-hidden="true" />
          {t('chat:resourcePicker.open')}
        </DropdownMenuItem>
        {visualAvailable && (
          <DropdownMenuItem
            data-composer-plus-item="visual"
            onSelect={() =>
              void openVisualPromptEntry({
                templatePrompt: t('chat:focusSession.visualPromptTemplate'),
                configureHint: t('chat:focusSession.configureVisualModelFirst'),
                notify,
                onOpenModelSettings,
                onInsertPrompt: applyWelcomeChip,
              })
            }
          >
            <ImagePlus aria-hidden="true" />
            {t('chat:focusSession.generateImage')}
          </DropdownMenuItem>
        )}
        {phonePage && botId ? null : phonePage ? (
          <DropdownMenuItem
            data-composer-plus-item="mode"
            onSelect={() => {
              afterPlusMenuCloseRef.current = () => setModeSheetOpen(true)
            }}
          >
            <Shield aria-hidden="true" />
            {t('board:composer.modeChip')}
            <DropdownMenuShortcut className="tracking-normal">
              {executionMode === 'full-access'
                ? t('chat:focusSession.fullAccessShort')
                : executionMode === 'workspace-write'
                  ? t('chat:focusSession.workspaceWriteShort')
                  : t('chat:focusSession.approvalRequiredShort')}
            </DropdownMenuShortcut>
          </DropdownMenuItem>
        ) : (
          <DropdownMenuItem
            data-composer-plus-item="voice"
            onSelect={() => voiceControlRef.current?.()}
          >
            <Mic aria-hidden="true" />
            {t('chat:voiceInput.open')}
            {VOICE_SHORTCUT && <DropdownMenuShortcut>{VOICE_SHORTCUT}</DropdownMenuShortcut>}
          </DropdownMenuItem>
        )}
        {draft?.plusMenuItems}
      </DropdownMenuContent>
    </DropdownMenu>
  )
  const modeChip = (
    <ComposerModeChip
      sheet={phonePage}
      collapse={phonePage}
      open={modeSheetOpen}
      onOpenChange={(open) => {
        setModeSheetOpen(open)
        // 手机上表单多半是从 + 菜单打开的（芯片收起）：关上后焦点回到 + 键，不丢到页面开头。
        if (!open && phonePage) window.setTimeout(() => plusTriggerRef.current?.focus(), 0)
      }}
      approvalMode={executionMode}
      onApprovalModeChange={onExecutionModeChange}
      approvalDisabled={switchingPermission}
      runMode={composerExecutionMode}
      onRunModeChange={changeExecutionMode}
      runModeDisabled={Boolean(streaming)}
      goalsAvailable={goalsAvailable}
      teamAvailable={teamAvailable}
      goal={goal}
      tokenBudget={composerExecutionMode === 'team' ? teamTokenBudget : goalTokenBudget}
      onTokenBudgetChange={
        composerExecutionMode === 'team' ? setTeamTokenBudget : setGoalTokenBudget
      }
      onSaveTokenBudget={(tokenBudget) => onGoalBudgetChange?.(tokenBudget)}
    />
  )
  const modelChip = (
    <ModelThinkingControl
      sheet={phonePage}
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
      usage={
        <ContextUsageSection
          usage={contextUsage}
          sessionUsage={sessionUsage}
          model={model}
          availableModels={availableModels}
          onThresholdChange={onCompactionThresholdChange}
        />
      }
    />
  )
  // 手机上模型胶囊只写“模型”，不参与收缩（窄屏露出完全访问芯片时由那个芯片截断，“模型”不被挤成竖排）。
  const modelChipSlot = (
    <div
      className={cn(
        'flex items-center',
        phonePage
          ? 'shrink-0'
          : 'min-w-0 shrink [&_.model-effort-pill]:max-w-full @max-[300px]/composer-footer:[&_.model-effort-pill>span:nth-child(n+2)]:hidden',
      )}
    >
      {modelChip}
    </div>
  )
  // 两种会话页头共用的数据与操作（菜单项都复用现有实现）。
  const pendingQuestions = useMemo(() => parsePendingQuestions(questions), [questions])
  const sessionMenuProps = {
    session,
    cwd: cwd || '',
    streaming: Boolean(streaming),
    treeAvailable: messages.length > 0,
    onRename,
    onWorkspace,
    workspaceDisabled: Boolean(streaming || switchingCwd),
    onCompact: onCompact ? () => void compactContext() : undefined,
    compactDisabled: Boolean(
      streaming || compactingManually || compaction?.active || messages.length === 0,
    ),
    goalAvailable: goalsAvailable,
    goalEngaged: composerExecutionMode === 'goal',
    goalDisabled: Boolean(streaming),
    onToggleGoal: () => changeExecutionMode(composerExecutionMode === 'goal' ? 'plan' : 'goal'),
    workflowsAvailable,
    onVoiceMode: () => setVoiceModeOpen(true),
    notify: notify ?? (() => undefined),
  }
  const pageHeaderBlock = pageChrome && (
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
      notify={notify}
    >
      {(_recallItem, tree) =>
        phonePage ? (
          <SessionPhoneHeader
            {...sessionMenuProps}
            runStartedAt={runStartedAt}
            approvals={approvals.length}
            failed={Boolean(!streaming && (error || latestAgentMessage?.error))}
            stopped={Boolean(!streaming && runStopped)}
            contextPercent={runMetricsContextPercent}
            leading={routeLeading}
            branches={tree.branches}
            onOpenTree={tree.open}
          />
        ) : (
          <SessionPageHeader
            {...sessionMenuProps}
            contextPercent={runMetricsContextPercent}
            branches={tree.branches}
            onOpenTree={tree.open}
          />
        )
      }
    </SessionTreeControl>
  )
  // 非路由的旧单会话顶栏已在 A6 退役：会话只在会话页（路由，带 pageChrome）里渲染，
  // 页头一律是 SessionPageHeader / SessionPhoneHeader；没有页面外壳时不画页头。
  const headerBlock = draft ? (
    <NewTaskDraftHeader
      phone={phonePage}
      leading={routeLeading}
      project={phonePage ? draft.renderProjectControl('header') : undefined}
    />
  ) : (
    pageHeaderBlock
  )
  const transcriptBlock = (
    <FocusTranscript
      emptyState={draft ? <NewTaskGreeting /> : emptyState}
      key="transcript"
      layoutMeasurementKey="fixed"
      sessionId={session.id}
      messages={transcriptMessages}
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
      collapseEarlierTurns={phonePage}
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
        // 这次提交的运行中方式读完即复位：发送键与 ↵ 都是排队，只有 ⌥↵ 是插话。
        const behavior = runningBehaviorRef.current
        runningBehaviorRef.current = 'steer'
        // “/btw 问题”不发给主会话：只清掉文字（附件留在草稿里），单独问一次；运行中也可以。
        const question = sideQuestionText(value)
        if (question !== null) {
          event.preventDefault()
          if (question) {
            sideQuestion.ask(question)
            updateValue('')
          }
          return
        }
        if (!streaming && (pendingRuntimeSelection || switchingModel || switchingThinking)) {
          event.preventDefault()
          return
        }
        void submit(event, pageChrome ? behavior : 'steer')
      }}
    >
      {sideQuestion.state && (
        <SideQuestionPanel
          state={sideQuestion.state}
          cwd={cwd || undefined}
          onDismiss={() => {
            sideQuestion.dismiss()
            promptRef.current?.focus()
          }}
        />
      )}
      {/* 内联批准卡与输入框同宽（规格 v2 第 4.2 节）：一屏只突出当前要处理的一张。 */}
      <ToolApproval
        approvals={approvals}
        onResolve={onApproval}
        sessionTitle={session.name || session.id}
        cwd={cwd || ''}
      />
      {pendingQuestions[0] && onAnswerQuestion && (
        // 代理的提问（规格 8.4）：与审批卡同一位置；审批在前时只留一行提示（一屏只突出一张）。
        <QuestionCard
          key={pendingQuestions[0].id}
          question={pendingQuestions[0]}
          more={pendingQuestions.length - 1}
          collapsed={approvals.length > 0}
          onAnswer={onAnswerQuestion}
        />
      )}
      {workflowsAvailable && !draft && <SessionWorkflowRuns sessionId={session.id} />}
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
      <div className="flex min-w-0 flex-col">
        <ComposerTray
          streaming={streaming}
          compaction={compaction}
          startedAt={runStartedAt}
          stopping={stopping}
          onStop={() => void stop()}
          runRow={!phonePage}
          queuedInputs={queuedInputs}
          withdrawingInputIds={withdrawingInputIds}
          onEdit={onWithdrawQueuedInput ? withdrawQueuedInput : undefined}
          onCancel={onWithdrawQueuedInput ? cancelQueuedInput : undefined}
          onSendNow={
            interruptAvailable && onSendQueuedInputNow
              ? (inputId) => void onSendQueuedInputNow(inputId)
              : undefined
          }
          // 手机不画排队列表：排队的消息就是对话里标着“排队中”的气泡（参考稿），托盘只剩压缩提示。
          queueList={!phonePage}
        />
        {draft?.notice && <div className="px-1">{draft.notice}</div>}
        {/* 输入卡片：圆角 22（守卫）+ 控件级描边（≥3:1），输入时焦点描边与 3px 焦点环。 */}
        <div
          className={cn(
            'focus-composer relative flex min-w-0 flex-col rounded-[var(--composer-radius,22px)] [corner-shape:var(--composer-corner-shape,round)] border border-control bg-solid shadow-composer transition-[border-color,box-shadow] duration-(--d-fast) ease-(--ease) has-[textarea:focus]:border-focus has-[textarea:focus]:ring-3 has-[textarea:focus]:ring-focus-ring max-[650px]:rounded-[28px] max-[650px]:border-stroke-strong max-[650px]:shadow-none max-[650px]:has-[textarea:focus]:border-control max-[650px]:has-[textarea:focus]:ring-0 [&>.attachment-tray]:px-3 [&>.attachment-tray]:pt-3 [&_textarea]:w-full [&_textarea]:min-w-0 [&_textarea]:min-h-11 [&_textarea]:max-h-[220px] [&_textarea]:resize-none [&_textarea]:overflow-y-auto [&_textarea]:border-0 [&_textarea]:[outline:0]! [&_textarea]:bg-transparent [&_textarea]:px-4 [&_textarea]:pt-3 [&_textarea]:pb-1 [&_textarea]:text-[length:calc(var(--app-message-font-size)*.9375)] max-[650px]:[&_textarea]:px-3.5 max-[650px]:[&_textarea]:text-[length:var(--app-message-font-size)] [&_textarea]:[font-family:var(--font-content-family)] [&_textarea]:font-[400] [&_textarea]:leading-5 [&_textarea]:text-content [&_textarea]:placeholder:text-content-muted',
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
          <AttachmentTray
            attachments={selection.attachments}
            preparing={selection.preparingAttachments}
            onRemove={selection.removeAttachment}
          />
          <ComposerCommandMenu
            placement="top"
            resourceKey={session.id}
            loadCommands={loadSessionCommands}
            builtinCommands={builtinCommands}
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
              // 会话页运行中 ⌥↵：插话（steer），代理在下一步就读到；普通 ↵ 仍走发送绑定 → 排队（followUp）。
              if (
                pageChrome &&
                streaming &&
                !composing &&
                steerShortcut &&
                matchesShortcut(event.nativeEvent, steerShortcut)
              ) {
                event.preventDefault()
                runningBehaviorRef.current = sendNowBehavior
                event.currentTarget.form?.requestSubmit()
                return
              }
              // ⇧Tab：在 Plan / Goal / Team 之间切换执行方式（规格 v2 第 2.8 节）。运行中、只有 Plan
              // 可用或带其他修饰键时不拦截，⇧Tab 照常把焦点移到上一个控件。
              if (
                event.key === 'Tab' &&
                event.shiftKey &&
                !event.altKey &&
                !event.ctrlKey &&
                !event.metaKey &&
                !composing &&
                !streaming
              ) {
                const nextMode = nextComposerRunMode(composerExecutionMode, {
                  goalsAvailable,
                  teamAvailable,
                })
                if (nextMode) {
                  event.preventDefault()
                  changeExecutionMode(nextMode)
                  setModeAnnouncement(
                    nextMode === 'team'
                      ? t('chat:focusSession.teamMode')
                      : nextMode === 'goal'
                        ? t('chat:focusSession.goalMode')
                        : t('chat:focusSession.planMode'),
                  )
                  return
                }
              }
              if (!composing && matchesShortcut(event.nativeEvent, shortcuts.sendMessage)) {
                event.preventDefault()
                event.currentTarget.form?.requestSubmit()
              }
              // 托盘写着「停止 Esc」：输入框内按 Esc 中断当前运行；命令菜单等已处理的 Esc 不再重复响应。
              // 会话页由页面级监听统一处理（先关浮层、再停止），这里不重复。
              if (
                !pageChrome &&
                streaming &&
                !composing &&
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
          <span role="status" className="sr-only">
            {modeAnnouncement}
          </span>
          {/* 工具行（规格 v2 第 4.3 节）：桌面 [+] [盾牌芯片] … [模型芯片] [发送]。
              手机照参考稿只留 [+] [模型] … [麦克风] [发送]：审批收进 + 菜单（完全访问或 Goal / Team 时芯片仍露出），
              新任务的项目在顶栏副标题里。控件都是 44 的圆形 / 胶囊：留 8px 内边距，与 28px 圆角大致同心。 */}
          <div
            // 手机上按钮是 36（触控 44），上下各多留 4px：输入框整体高度和按钮中心位置都和 44 按钮时一样，只是按钮变小。
            className="focus-composer-footer @container/composer-footer flex min-w-0 items-center gap-1 px-2 pt-1 pb-2 max-[650px]:gap-2 max-[650px]:pt-2 max-[650px]:pb-3"
            data-composer-toolbar
          >
            {plusMenu}
            {attachmentChooser.input}
            {phonePage && !botId && modelChipSlot}
            {!phonePage && draft?.renderProjectControl('chip')}
            {!botId && modeChip}
            <span className="min-w-0 flex-1" />
            {voiceInput}
            {!phonePage && !botId && modelChipSlot}
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
              onLongPress={phonePage && streaming ? submitSteer : undefined}
              longPressHint={t('board:composer.longPressSteer')}
              onStop={phonePage ? () => void stop() : undefined}
            />
          </div>
        </div>
      </div>
    </form>
  )
  return (
    <Panel
      className={cn(
        `focus-session [--chat-content-width:768px] [.session-dock-panel_&]:overflow-hidden [.session-dock-panel_&]:min-h-0 [.session-dock-panel_&]:border-0 [.session-dock-panel_&]:rounded-[0] [.session-dock-panel_&]:bg-[var(--panel)] [.session-dock-panel_&]:p-0 [.session-dock-panel_&]:shadow-[none] [[data-theme='dark']_.session-dock-panel_&]:bg-[var(--main-surface-bg)] min-[651px]:[[data-density='compact']_.app-card:not(&)]:p-[10px] max-[650px]:min-h-[460px] max-[650px]:[.session-dock-panel_&]:min-h-0 relative flex h-full min-h-0 flex-col ${hasConversation ? 'has-conversation' : 'is-empty'}`,
        // 新任务草稿（桌面）：问候语与输入框一起放在页面中部（新对话的常见做法）；手机上输入框仍停靠底部。
        draft &&
          'min-[651px]:[&_[data-chat-region=messages]]:!flex-none min-[651px]:[&_[data-chat-region=messages]]:mt-[max(48px,calc((100dvh-520px)/2))] min-[651px]:[&_[data-chat-region=messages]]:!overflow-visible min-[651px]:[&_.transcript]:!overflow-visible min-[651px]:[&_.transcript]:!p-0 min-[651px]:[&_.transcript]:!pb-6',
      )}
      data-chat-message-style="bubble"
    >
      <div
        ref={panelRootRef}
        className="flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      >
        <div className="shrink-0" data-chat-region="header">
          {slots?.headerHost
            ? createPortal(headerBlock, slots.headerHost)
            : !slots
              ? headerBlock
              : null}
        </div>
        <div
          className="flex min-h-0 flex-1 flex-col overflow-hidden"
          data-chat-region="messages"
          data-focus-region="content"
        >
          {transcriptMeta ? (
            <TranscriptMetaContext.Provider value={transcriptMeta}>
              {transcriptBlock}
            </TranscriptMetaContext.Provider>
          ) : (
            transcriptBlock
          )}
        </div>
        <div className="shrink-0" data-chat-region="composer" data-focus-region="composer">
          {composerBlock}
        </div>
      </div>
      {slots?.sourceActionHost &&
        createPortal(<AttachmentPicker cwd={cwd} selection={selection} />, slots.sourceActionHost)}
      <ChatResourcePicker
        open={resourcePickerOpen}
        sessionId={session.id}
        cwd={cwd || ''}
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
