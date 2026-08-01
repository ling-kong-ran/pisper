import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type UIEvent,
} from 'react'
import { createPortal } from 'react-dom'
import {
  AlertTriangle,
  ArrowDown,
  Bot,
  Check,
  Command,
  Eye,
  File,
  FolderOpen,
  Gauge,
  MoreHorizontal,
  PanelBottom,
  PanelLeft,
  PanelRight,
  PanelTop,
  Paperclip,
  Pencil,
  RefreshCw,
  Send,
  Shield,
  ShieldCheck,
  ShieldOff,
  Square,
  Target,
  X,
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { I18nValues } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { Confirmation } from '@/components/ai-elements/confirmation'
import { QueueSection } from '@/components/ai-elements/queue'
import { BrandLogo } from '@/components/BrandLogo'
import { AsciiText, Aurora, BlurText, TargetCursor } from '@/components/react-bits'
import { Panel, Toggle } from '@/components/ui'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useAutoScroll } from '@/hooks/useAutoScroll'
import { formatFileSize, formatTokenCount, workspaceName } from '@/lib/format'
import { resolveMessageRunActivity } from '@/lib/session-state'
import type {
  ChatAttachment,
  ChatMessage,
  EntityRecord,
  ModelOption,
  SessionSummary,
  TaskList,
} from '@/types/chat'
import { useAttachmentSelection } from './attachments'
import { FocusChatMessage } from './ChatMessage'
import { requestCommandPalette } from './events'
import { GitChangesControl } from './GitChangesControl'
import { activityScrollVersion } from './run-activity'
import TaskBoard from './TaskBoard'

type Translate = (message: string, values?: I18nValues) => string
type ExecutionModeOption = [string, string, string, LucideIcon]

const MIN_GOAL_TOKEN_BUDGET = 1_000
const DEFAULT_GOAL_TOKEN_BUDGET = 30_000
const COMMAND_PALETTE_SHORTCUT =
  typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform)
    ? '\u2318 K'
    : 'Ctrl K'

type SessionModelSelectProps = {
  value: string
  models: ModelOption[]
  onChange: (model: string) => void
  disabled?: boolean
  compact?: boolean
}

type ExecutionModeSelectProps = {
  value: string
  onChange: (mode: string) => void
  disabled?: boolean
  compact?: boolean
}

type ToolApprovalProps = {
  approvals: EntityRecord[]
  onResolve: (approvalId: string, approved: boolean) => Promise<void> | void
  compact?: boolean
}

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
  taskList?: TaskList | null
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

function ContextUsageIndicator({
  usage,
  onThresholdChange,
}: {
  usage?: EntityRecord | null
  onThresholdChange?: (thresholdPercent: number) => Promise<void> | void
}) {
  const { t } = useI18n()
  const contextWindow = Number(usage?.contextWindow) || 0
  const compactAtPercent = usage?.compactAtPercent == null ? 80 : Number(usage.compactAtPercent)
  const currentThreshold = Number.isFinite(compactAtPercent) ? Math.round(compactAtPercent) : 80
  const [draftThreshold, setDraftThreshold] = useState(currentThreshold)
  const [savingThreshold, setSavingThreshold] = useState(false)
  const [thresholdError, setThresholdError] = useState('')
  const lastSavedThreshold = useRef(currentThreshold)
  const thresholdSaveTimer = useRef<number | undefined>(undefined)

  useEffect(() => {
    window.clearTimeout(thresholdSaveTimer.current)
    setDraftThreshold(currentThreshold)
    lastSavedThreshold.current = currentThreshold
  }, [currentThreshold])

  useEffect(() => () => window.clearTimeout(thresholdSaveTimer.current), [])

  if (!contextWindow) return null
  const known = usage?.percent != null && Number.isFinite(Number(usage.percent))
  const percent = known ? Math.max(0, Number(usage.percent)) : null
  const roundedPercent = percent == null ? null : Math.round(percent)
  const warningAt = Math.max(50, currentThreshold - 15)
  const tone =
    percent == null
      ? 'unknown'
      : percent >= currentThreshold
        ? 'danger'
        : percent >= warningAt
          ? 'warning'
          : 'normal'
  const usageText = known
    ? usage.estimated
      ? t('chat:focusSession.estimatedContextUsageTokensLimitTokensPercent', {
          tokens: formatTokenCount(usage.tokens),
          limit: formatTokenCount(contextWindow),
          percent: roundedPercent,
        })
      : t('chat:focusSession.contextUsageTokensLimitTokensPercent', {
          tokens: formatTokenCount(usage.tokens),
          limit: formatTokenCount(contextWindow),
          percent: roundedPercent,
        })
    : t('chat:focusSession.contextUsageWillUpdateAfterTheNextModelResponseLimitLimitTokens', {
        limit: formatTokenCount(contextWindow),
      })
  const thresholdText = usage?.autoCompactEnabled
    ? t('chat:focusSession.autoCompactionThresholdAboutPercent', {
        percent: currentThreshold,
      })
    : t('chat:focusSession.automaticContextCompactionIsDisabled')
  const label = `${usageText} · ${thresholdText}`
  const tokenLabel = `${usage?.tokens == null ? '—' : formatTokenCount(usage.tokens)} / ${formatTokenCount(contextWindow)}`

  const saveThreshold = async (value: number) => {
    const next = Math.min(95, Math.max(50, Math.round(value)))
    setDraftThreshold(next)
    if (!onThresholdChange || next === lastSavedThreshold.current) return
    setSavingThreshold(true)
    setThresholdError('')
    try {
      await onThresholdChange(next)
      lastSavedThreshold.current = next
    } catch (error) {
      setThresholdError(error instanceof Error ? error.message : String(error))
    } finally {
      setSavingThreshold(false)
    }
  }

  const scheduleThresholdSave = (value: number) => {
    window.clearTimeout(thresholdSaveTimer.current)
    setDraftThreshold(value)
    thresholdSaveTimer.current = window.setTimeout(() => void saveThreshold(value), 250)
  }

  const commitThreshold = (value: number) => {
    window.clearTimeout(thresholdSaveTimer.current)
    void saveThreshold(value)
  }

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={`context-usage-chip ${tone}`}
          aria-label={label}
          title={label}
        >
          <Gauge size={12} />
          <span>
            <strong>{tokenLabel}</strong>
            <small>{roundedPercent == null ? '—' : `${roundedPercent}%`}</small>
          </span>
          <i aria-hidden="true">
            <b style={{ width: `${Math.min(100, percent || 0)}%` }} />
          </i>
        </button>
      </PopoverTrigger>
      <PopoverContent className="context-usage-popover" align="end" sideOffset={8}>
        <div className="context-threshold-heading">
          <span>{t('chat:focusSession.autoCompactionThreshold')}</span>
          <output>{draftThreshold}%</output>
        </div>
        <input
          type="range"
          min="50"
          max="95"
          step="1"
          value={draftThreshold}
          aria-label={t('chat:focusSession.autoCompactionThreshold')}
          disabled={savingThreshold}
          onChange={(event) => scheduleThresholdSave(Number(event.currentTarget.value))}
          onBlur={(event) => commitThreshold(Number(event.currentTarget.value))}
          onPointerUp={(event) => commitThreshold(Number(event.currentTarget.value))}
          onKeyUp={(event) => {
            if (
              ['ArrowLeft', 'ArrowRight', 'Home', 'End', 'PageUp', 'PageDown'].includes(event.key)
            ) {
              commitThreshold(Number(event.currentTarget.value))
            }
          }}
        />
        <div className="context-threshold-scale" aria-hidden="true">
          <span>50%</span>
          <span>95%</span>
        </div>
        <small className={thresholdError ? 'error' : ''}>
          {thresholdError ||
            (savingThreshold ? t('chat:focusSession.savingCompactionThreshold') : '\u00a0')}
        </small>
      </PopoverContent>
    </Popover>
  )
}

function SessionModelSelect({
  value,
  models,
  onChange,
  disabled,
  compact = false,
}: SessionModelSelectProps) {
  const { t } = useI18n()
  const currentModel = models.find((model) => model.key === value)
  const hasCurrentModel = Boolean(currentModel)
  const currentLabel = currentModel
    ? `${currentModel.providerName} · ${currentModel.label}`
    : value.split('/').at(-1)
  return (
    <div
      className={`session-model-select icon-only ${compact ? 'compact' : ''}`}
      title={
        disabled
          ? t('chat:focusSession.currentModelModelCannotSwitchWhileRunning', {
              model: currentLabel,
            })
          : t('chat:focusSession.currentModelModelClickToSwitch', {
              model: currentLabel,
            })
      }
    >
      <Bot size={compact ? 11 : 14} />
      <AppSelect
        value={value}
        onChange={(event) => onChange(event.target.value)}
        disabled={disabled || models.length === 0}
        aria-label={t('chat:focusSession.currentChatModel')}
      >
        {!hasCurrentModel && <option value={value}>{value.split('/').at(-1)}</option>}
        {models.map((model) => (
          <option key={model.key} value={model.key}>
            {model.providerName} · {model.label}
          </option>
        ))}
      </AppSelect>
    </div>
  )
}

function welcomeChips(t: Translate) {
  return [
    {
      label: t('chat:focusSession.explainCode'),
      prompt: t('chat:focusSession.explainHowThisCodeWorks'),
    },
    {
      label: t('chat:focusSession.writeTests'),
      prompt: t('chat:focusSession.writeUnitTestsForTheFollowingCode'),
    },
    {
      label: t('chat:focusSession.refactor'),
      prompt: t('chat:focusSession.refactorThisCodeAndExplainTheImprovements'),
    },
    {
      label: t('chat:focusSession.findABug'),
      prompt: t('chat:focusSession.helpMeLocateAndFixThisBug'),
    },
  ]
}

function executionModeOptions(t: Translate): ExecutionModeOption[] {
  return [
    [
      'read-only',
      t('chat:focusSession.readOnly'),
      t('chat:focusSession.onlyInspectAndAnalyzeCode'),
      Eye,
    ],
    [
      'workspace',
      t('chat:focusSession.workspace'),
      t('chat:focusSession.workspaceReadsDirectlyAndApprovesWritesAndShellCommands'),
      Shield,
    ],
    [
      'full-access',
      t('chat:focusSession.fullAccess'),
      t('chat:focusSession.fullAccessRunsShellWithoutPerCommandApproval'),
      ShieldOff,
    ],
  ]
}

function ExecutionModeSelect({
  value,
  onChange,
  disabled,
  compact = false,
}: ExecutionModeSelectProps) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [menuPosition, setMenuPosition] = useState({ left: 0, top: 0, width: 270 })
  const rootRef = useRef<HTMLDivElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const options = executionModeOptions(t)
  const current = options.find((item) => item[0] === value) || options[1]
  const CurrentIcon = current[3]
  const positionMenu = useCallback(() => {
    const trigger = rootRef.current?.querySelector('button')
    if (!trigger) return
    const rect = trigger.getBoundingClientRect()
    const edge = 8
    const gap = 8
    const width = Math.min(270, window.innerWidth - edge * 2)
    const height = menuRef.current?.offsetHeight || 190
    const left = Math.max(edge, Math.min(rect.right - width, window.innerWidth - width - edge))
    const top =
      rect.top >= height + gap + edge
        ? rect.top - height - gap
        : Math.min(rect.bottom + gap, window.innerHeight - height - edge)
    setMenuPosition({ left, top: Math.max(edge, top), width })
  }, [])
  useLayoutEffect(() => {
    if (!open) return undefined
    positionMenu()
    window.addEventListener('resize', positionMenu)
    window.addEventListener('scroll', positionMenu, true)
    return () => {
      window.removeEventListener('resize', positionMenu)
      window.removeEventListener('scroll', positionMenu, true)
    }
  }, [open, positionMenu])
  useEffect(() => {
    if (!open) return undefined
    const close = (event: MouseEvent) => {
      const target = event.target instanceof Node ? event.target : null
      if (!rootRef.current?.contains(target) && !menuRef.current?.contains(target)) setOpen(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape)
    }
  }, [open])
  const menu =
    open &&
    createPortal(
      <div
        ref={menuRef}
        className="permission-mode-menu execution-mode-menu !fixed !right-auto !bottom-auto z-[80]"
        style={menuPosition}
        role="menu"
      >
        <div className="execution-mode-menu-title">{t('chat:focusSession.executionMode')}</div>
        {options.map(([mode, label, description, Icon]) => (
          <button
            type="button"
            role="menuitemradio"
            aria-checked={mode === current[0]}
            className={mode === current[0] ? 'active' : ''}
            onClick={() => {
              onChange(mode)
              setOpen(false)
            }}
            key={mode}
          >
            <span className={`permission-level level-${mode}`}>
              <Icon size={13} />
            </span>
            <span>
              <strong>
                {label}
                {mode === 'workspace' && (
                  <small className="recommended-mode">{t('chat:focusSession.recommended')}</small>
                )}
              </strong>
              <small>{description}</small>
            </span>
            {mode === current[0] && <Check size={13} />}
          </button>
        ))}
      </div>,
      document.body,
    )
  return (
    <>
      <div
        ref={rootRef}
        className={`permission-mode-select execution-mode-select icon-only ${compact ? 'compact' : ''} ${open ? 'open' : ''}`}
      >
        <button
          type="button"
          className={`permission-mode-trigger icon-only mode-${current[0]}`}
          title={t('chat:focusSession.executionModeModeDescription', {
            mode: current[1],
            description: current[2],
          })}
          disabled={disabled}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={t('chat:focusSession.executionModeMode', { mode: current[1] })}
          onClick={() => setOpen((visible) => !visible)}
        >
          <CurrentIcon size={compact ? 11 : 14} />
        </button>
      </div>
      {menu}
    </>
  )
}

function ToolApproval({ approvals, onResolve, compact = false }: ToolApprovalProps) {
  const { t } = useI18n()
  const [resolving, setResolving] = useState(false)
  const resolvingRef = useRef(false)
  const approval = approvals[0]
  if (!approval) return null
  const resolve = async (approved: boolean) => {
    if (resolvingRef.current) return
    resolvingRef.current = true
    setResolving(true)
    try {
      await onResolve(approval.id, approved)
    } finally {
      resolvingRef.current = false
      setResolving(false)
    }
  }
  return (
    <Confirmation
      approval={{ id: approval.id }}
      state="approval-requested"
      className={`tool-approval ${compact ? 'compact' : ''}`}
      data-pisper-approval-id={approval.id}
    >
      <div>
        <ShieldCheck size={compact ? 12 : 15} />
        <span>
          <strong>
            {t('chat:focusSession.toolRequestsApproval', {
              tool: approval.toolName,
            })}
          </strong>
          <small>
            {approval.reason}
            {approvals.length > 1
              ? ` · ${t('chat:focusSession.countMoreWaiting', { count: approvals.length - 1 })}`
              : ''}
          </small>
        </span>
      </div>
      {!compact && (
        <details>
          <summary>{t('chat:focusSession.viewCallArguments')}</summary>
          <pre>{JSON.stringify(approval.args, null, 2)}</pre>
        </details>
      )}
      <div className="tool-approval-actions">
        <button
          type="button"
          className="button secondary"
          disabled={resolving}
          onClick={() => resolve(false)}
        >
          {t('chat:focusSession.deny')}
        </button>
        <button
          type="button"
          className="button primary"
          disabled={resolving}
          onClick={() => resolve(true)}
        >
          {resolving ? <RefreshCw className="spin" size={12} /> : <Check size={12} />}
          {t('chat:focusSession.allow')}
        </button>
      </div>
    </Confirmation>
  )
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
  taskList,
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
  const selection = useAttachmentSelection()
  const addSelectedAttachments = selection.addAttachments
  const promptRef = useRef<HTMLTextAreaElement>(null)
  const prependSnapshot = useRef<{ scrollHeight: number; scrollTop: number } | null>(null)
  const lastMessage = messages[messages.length - 1]
  // Bucket streaming text length so auto-scroll does not fire on every token.
  const textScrollBucket = Math.floor((lastMessage?.text?.length || 0) / 64)
  const activityVersion = activityScrollVersion(activityFeed)
  const thinkingScrollBucket = Math.floor(String(thinkingText || '').length / 128)
  const transcriptVersion = `${session?.id || ''}:${lastMessage?.id || ''}:${textScrollBucket}:${thinkingScrollBucket}:${lastMessage?.attachments?.length || 0}:${activityVersion}:${goal?.status || ''}:${goal?.tokensUsed || 0}:${compaction?.status || ''}:${compaction?.finishedAt || ''}:${error || ''}:${streaming ? '1' : '0'}`
  const {
    scrollRef: transcriptRef,
    onScroll: onTranscriptScroll,
    hasUnread,
    scrollToBottom,
  } = useAutoScroll(transcriptVersion)
  // Mirror the pinned-to-bottom state so the post-stream catch-up scroll below
  // does not yank a reader who has scrolled up to review earlier output.
  const pinnedToBottomRef = useRef(true)
  useEffect(() => {
    pinnedToBottomRef.current = !hasUnread
  }, [hasUnread])
  // When streaming ends, the message swaps from the plain <pre> streaming view
  // to full ReactMarkdown (tables, code blocks, syntax highlighting). That
  // height change lands *after* the final auto-scroll has already fired against
  // a stale scrollHeight, leaving the tail off-screen. Re-scroll once the layout
  // settles, but only if the user is still at the bottom.
  useEffect(() => {
    if (streaming) return undefined
    if (!pinnedToBottomRef.current) return undefined
    let outer = 0
    let inner = 0
    let timeout = 0
    outer = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => scrollToBottom())
      // Fallback for layout that settles past two frames (async reflow).
      timeout = window.setTimeout(() => scrollToBottom(), 60)
    })
    return () => {
      cancelAnimationFrame(outer)
      if (inner) cancelAnimationFrame(inner)
      if (timeout) window.clearTimeout(timeout)
    }
  }, [streaming, scrollToBottom])
  const latestRunProps = useMemo(
    () => ({
      streaming,
      text: lastMessage?.role === 'agent' ? lastMessage.text : '',
      currentActivity,
      activityFeed,
      tools,
      thinkingText,
      compaction,
      error: error || (lastMessage?.role === 'agent' ? lastMessage.error : ''),
      stopped: runStopped,
      notice: runNotice,
      startedAt: runStartedAt,
      lastActivityAt,
      finishedAt: runFinishedAt,
    }),
    [
      streaming,
      lastMessage,
      currentActivity,
      activityFeed,
      tools,
      thinkingText,
      compaction,
      error,
      runStopped,
      runNotice,
      runStartedAt,
      lastActivityAt,
      runFinishedAt,
    ],
  )
  const loadOlder = useCallback(async () => {
    const node = transcriptRef.current
    if (!node || !hasOlder || loadingOlder || prependSnapshot.current) return
    prependSnapshot.current = { scrollHeight: node.scrollHeight, scrollTop: node.scrollTop }
    const loaded = await onLoadOlder?.()
    if (!loaded) prependSnapshot.current = null
  }, [hasOlder, loadingOlder, onLoadOlder, transcriptRef])
  const handleTranscriptScroll = useCallback(
    (event: UIEvent<HTMLDivElement>) => {
      onTranscriptScroll(event)
      if (event.currentTarget.scrollTop <= 96) void loadOlder()
    },
    [loadOlder, onTranscriptScroll],
  )
  useLayoutEffect(() => {
    const snapshot = prependSnapshot.current
    const node = transcriptRef.current
    if (!snapshot || !node) return
    node.scrollTop = snapshot.scrollTop + node.scrollHeight - snapshot.scrollHeight
    prependSnapshot.current = null
  }, [messageStart, transcriptRef])
  useEffect(() => {
    setGoalArmed(false)
    setQueueing(false)
  }, [session?.id])
  useEffect(() => {
    if (!pendingAsset) return
    addSelectedAttachments([pendingAsset])
    onAssetConsumed?.()
  }, [pendingAsset, onAssetConsumed, addSelectedAttachments])
  const applyWelcomeChip = (prompt: string) => {
    setValue(prompt)
    requestAnimationFrame(() => {
      const el = promptRef.current
      if (!el) return
      el.focus()
      el.style.height = 'auto'
      el.style.height = `${Math.min(el.scrollHeight, 220)}px`
    })
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
      requestAnimationFrame(() => scrollToBottom('smooth'))
      if (promptRef.current) promptRef.current.style.height = 'auto'
      return
    }
    onSend(value, selection.attachments, goalArmed, goalArmed ? goalTokenBudget : null)
    scrollToBottom('smooth')
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
      <div className="transcript" ref={transcriptRef} onScroll={handleTranscriptScroll}>
        {(hasOlder || loadingOlder || olderError) && (
          <div className="history-page-loader">
            {olderError ? (
              <button type="button" className="button secondary" onClick={loadOlder}>
                <RefreshCw size={13} />
                {t('chat:focusSession.retryOlderMessages')}
              </button>
            ) : loadingOlder ? (
              <>
                <RefreshCw className="spin" size={14} />
                {t('chat:focusSession.loadingOlderMessages')}
              </>
            ) : (
              <button type="button" className="button secondary" onClick={loadOlder}>
                <ArrowDown className="history-up-arrow" size={14} />
                {t('chat:focusSession.loadOlderMessages')}
              </button>
            )}
          </div>
        )}
        {taskList?.items?.length ? <TaskBoard taskList={taskList} /> : null}
        {!messages.length && (
          <div className="agent-welcome">
            <Aurora />
            <TargetCursor className="agent-welcome-content">
              <div className="welcome-visual">
                <BrandLogo size={54} className="welcome-logo" />
                <AsciiText text="PISPER" />
              </div>
              <h2>
                <BlurText text={t('chat:focusSession.letSBeginWithASparkOfAnIdea')} />
              </h2>
              <p>
                {t(
                  'chat:focusSession.pisperIsReadyToReadTheCurrentWorkspaceSearchTheCodebaseAndHelpCarryTheTaskThroughItRunsInTheWork',
                )}
              </p>
              <div className="welcome-chips">
                {welcomeChips(t).map((chip) => (
                  <button
                    type="button"
                    key={chip.label}
                    data-target-cursor
                    onClick={() => applyWelcomeChip(chip.prompt)}
                  >
                    {chip.label}
                  </button>
                ))}
              </div>
            </TargetCursor>
          </div>
        )}
        {messages.map((message, index) => {
          const isLatestAgent = message.role === 'agent' && index === messages.length - 1
          const agentState =
            message.streaming || (isLatestAgent && streaming)
              ? 'thinking'
              : isLatestAgent && !message.error
                ? 'waiting'
                : 'idle'
          const runProps = resolveMessageRunActivity(message, isLatestAgent, latestRunProps)
          return (
            <FocusChatMessage
              key={message.id}
              message={message}
              agentState={agentState}
              showRunActivity={Boolean(runProps)}
              runProps={runProps}
            />
          )
        })}
        {error && (
          <div className="chat-error">
            <AlertTriangle size={14} />
            {error}
          </div>
        )}
      </div>
      {hasUnread && (
        <button
          type="button"
          className="button secondary jump-to-latest"
          onClick={() => scrollToBottom('smooth')}
        >
          <ArrowDown size={14} />
          {t('chat:focusSession.newContent')}
        </button>
      )}
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
            <GitChangesControl sessionId={session?.id} streaming={streaming} />
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

function GoalModeControl({
  goal,
  armed,
  tokenBudget,
  onTokenBudgetChange,
  onSaveTokenBudget,
  onChange,
}: {
  goal?: EntityRecord | null
  armed: boolean
  tokenBudget: number
  onTokenBudgetChange: (tokenBudget: number) => void
  onSaveTokenBudget?: (tokenBudget: number) => Promise<void> | void
  onChange: (enabled: boolean) => void
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [savingBudget, setSavingBudget] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const active = goal?.status === 'active'
  const enabled = active || armed
  const hasExistingGoal = Boolean(goal?.id)
  const currentBudget = Number(goal?.tokenBudget) || DEFAULT_GOAL_TOKEN_BUDGET
  const budgetDirty = tokenBudget !== currentBudget
  const status = active
    ? t('chat:focusSession.runningAutonomously')
    : armed
      ? goal?.status === 'paused'
        ? t('chat:focusSession.theNextMessageWillResumeTheGoal')
        : t('chat:focusSession.theNextMessageWillStartAGoal')
      : goal?.status === 'complete'
        ? t('chat:focusSession.goalComplete')
        : goal?.status === 'budget_limited'
          ? t('chat:focusSession.goalReachedItsBudget')
          : goal?.status === 'paused'
            ? t('chat:focusSession.goalPaused')
            : t('chat:focusSession.enableForTheNextMessageOnly')
  const objective = String(goal?.objective || '')
    .replace(/\s+/g, ' ')
    .trim()
  const detail = armed ? status : objective || status
  const usage = hasExistingGoal
    ? t('chat:focusSession.usedBudgetTokensUsed', {
        used: goal?.tokensUsed || 0,
        budget: goal?.tokenBudget || 0,
      })
    : ''
  const label = [t('chat:focusSession.goalMode'), detail, usage].filter(Boolean).join(' · ')

  useEffect(() => {
    if (!open) return undefined
    const close = (event: MouseEvent) => {
      const target = event.target instanceof Node ? event.target : null
      if (!rootRef.current?.contains(target)) setOpen(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  const change = (next: boolean) => {
    onChange(next)
    setOpen(false)
  }

  const saveBudget = async () => {
    if (savingBudget || !budgetDirty) return
    if (hasExistingGoal && onSaveTokenBudget) {
      setSavingBudget(true)
      try {
        await onSaveTokenBudget(tokenBudget)
      } finally {
        setSavingBudget(false)
      }
    }
  }

  return (
    <div
      ref={rootRef}
      className={`goal-mode-select ${open ? 'open' : ''} ${active || armed ? 'active' : ''}`}
    >
      <button
        type="button"
        className="goal-mode-trigger"
        title={label}
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((visible) => !visible)}
      >
        <Target size={14} />
      </button>
      {open && (
        <div className="goal-mode-menu" role="dialog" aria-label={t('chat:focusSession.goalMode')}>
          <div className="goal-mode-menu-row">
            <span className="goal-mode-menu-icon">
              <Target size={15} />
            </span>
            <span>
              <strong>{t('chat:focusSession.goalMode')}</strong>
              <small title={detail}>{detail}</small>
            </span>
            <Toggle value={enabled} onChange={change} ariaLabel={label} title={label} />
          </div>
          {usage && <p>{usage}</p>}
          <div className="goal-mode-budget-row">
            <label htmlFor="goal-token-budget-input">
              {t('chat:focusSession.goalTokenBudget')}
            </label>
            <input
              id="goal-token-budget-input"
              type="number"
              min={MIN_GOAL_TOKEN_BUDGET}
              step={1000}
              value={tokenBudget}
              disabled={savingBudget}
              onChange={(event) => {
                const next = Number(event.target.value)
                if (!Number.isFinite(next)) return
                onTokenBudgetChange(Math.max(MIN_GOAL_TOKEN_BUDGET, Math.round(next)))
              }}
            />
            {hasExistingGoal && budgetDirty && (
              <button
                type="button"
                className="button secondary tiny"
                disabled={savingBudget}
                onClick={() => void saveBudget()}
              >
                {savingBudget ? <RefreshCw className="spin" size={12} /> : <Check size={12} />}
                {t('chat:focusSession.goalBudgetSave')}
              </button>
            )}
          </div>
          <small className="goal-mode-budget-hint">
            {hasExistingGoal
              ? t('chat:focusSession.goalTokenBudgetUpdateHint')
              : t('chat:focusSession.goalTokenBudgetHint', {
                  min: formatTokenCount(MIN_GOAL_TOKEN_BUDGET),
                })}
          </small>
        </div>
      )}
    </div>
  )
}

function SessionActionsMenu({
  session,
  canSplit,
  streaming,
  switchingCwd,
  onSplitLeft,
  onSplitRight,
  onSplitTop,
  onSplitBottom,
  onClosePanel,
  onWorkspace,
  onRename,
}: {
  session: SessionSummary
  canSplit?: boolean
  streaming?: boolean
  switchingCwd?: boolean
  onSplitLeft: () => void
  onSplitRight: () => void
  onSplitTop: () => void
  onSplitBottom: () => void
  onClosePanel: () => void
  onWorkspace: () => void
  onRename: () => void
}) {
  const { t, language } = useI18n()
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return undefined
    const close = (event: MouseEvent) => {
      const target = event.target instanceof Node ? event.target : null
      if (!rootRef.current?.contains(target)) setOpen(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  const run = (action?: () => void) => {
    setOpen(false)
    action?.()
  }

  return (
    <div ref={rootRef} className="session-actions-menu-root">
      <button
        type="button"
        className="icon-button"
        title={t('chat:focusSession.chatActions')}
        aria-label={t('chat:focusSession.openChatActionsMenu')}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={!session}
        onClick={() => setOpen((visible) => !visible)}
      >
        <MoreHorizontal size={17} />
      </button>
      {open && (
        <div className="permission-mode-menu session-actions-menu" role="menu">
          <button
            type="button"
            role="menuitem"
            disabled={!canSplit}
            onClick={() => run(onSplitLeft)}
          >
            <PanelLeft size={15} />
            <span>
              <strong>{t('chat:focusSession.splitToLeft')}</strong>
              <small>
                {canSplit
                  ? t('chat:focusSession.moveTheCurrentTabIntoANewGroupOnTheLeft')
                  : t('chat:focusSession.thisGroupHasOnlyOneChat')}
              </small>
            </span>
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={!canSplit}
            onClick={() => run(onSplitRight)}
          >
            <PanelRight size={15} />
            <span>
              <strong>{t('chat:focusSession.splitToRight')}</strong>
              <small>
                {canSplit
                  ? t('chat:focusSession.moveTheCurrentTabIntoANewGroupOnTheRight')
                  : t('chat:focusSession.thisGroupHasOnlyOneChat')}
              </small>
            </span>
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={!canSplit}
            onClick={() => run(onSplitTop)}
          >
            <PanelTop size={15} />
            <span>
              <strong>{t('chat:focusSession.splitToTop')}</strong>
              <small>
                {canSplit
                  ? t('chat:focusSession.moveTheCurrentTabIntoANewGroupOnTheTop')
                  : t('chat:focusSession.thisGroupHasOnlyOneChat')}
              </small>
            </span>
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={!canSplit}
            onClick={() => run(onSplitBottom)}
          >
            <PanelBottom size={15} />
            <span>
              <strong>{t('chat:focusSession.splitToBottom')}</strong>
              <small>
                {canSplit
                  ? t('chat:focusSession.moveTheCurrentTabIntoANewGroupOnTheBottom')
                  : t('chat:focusSession.thisGroupHasOnlyOneChat')}
              </small>
            </span>
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={streaming || switchingCwd}
            onClick={() => run(onWorkspace)}
          >
            <FolderOpen size={15} />
            <span>
              <strong>{t('chat:focusSession.setWorkingDirectory')}</strong>
              <small>
                {streaming
                  ? t('chat:focusSession.cannotSwitchWhileTheAgentIsRunning')
                  : workspaceName(session?.cwd, language)}
              </small>
            </span>
          </button>
          <button type="button" role="menuitem" onClick={() => run(onRename)}>
            <Pencil size={15} />
            <span>
              <strong>{t('chat:focusSession.renameChat')}</strong>
              <small>{session?.name || t('chat:focusSession.newChat')}</small>
            </span>
          </button>
          <button type="button" role="menuitem" onClick={() => run(onClosePanel)}>
            <X size={15} />
            <span>
              <strong>{t('chat:focusSession.closeTab')}</strong>
              <small>{t('chat:focusSession.keepTheHistoryAndCloseOnlyThisView')}</small>
            </span>
          </button>
        </div>
      )}
    </div>
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
