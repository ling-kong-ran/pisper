import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Bot, Brain, Check, Database, Eye, Gauge, Pencil, ShieldOff, Sigma } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { I18nValues } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { formatTokenCount } from '@/lib/format'
import type { EntityRecord, ModelOption } from '@/types/chat'

type Translate = (message: string, values?: I18nValues) => string
type ExecutionModeOption = [string, string, string, LucideIcon]

export function SessionUsageMetrics({ usage }: { usage?: EntityRecord | null }) {
  const { t } = useI18n()
  const totalTokens = Math.max(0, Number(usage?.totalTokens) || 0)
  const cacheHitRate = Number(usage?.cacheHitRate)
  const cacheRateKnown = usage?.cacheHitRate != null && Number.isFinite(cacheHitRate)
  const cacheRateLabel = cacheRateKnown ? `${Math.round(Math.max(0, cacheHitRate))}%` : '—'
  const title = t('chat:focusSession.sessionUsageDetail', {
    input: formatTokenCount(usage?.input),
    output: formatTokenCount(usage?.output),
    cacheRead: formatTokenCount(usage?.cacheRead),
    cacheWrite: formatTokenCount(usage?.cacheWrite),
    reasoning: formatTokenCount(usage?.reasoning),
    requests: Math.max(0, Number(usage?.requests) || 0),
  })

  return (
    <div className="session-usage-metrics" title={title} aria-label={title}>
      <span>
        <Database size={12} />
        <small>{t('chat:focusSession.cacheHitRate')}</small>
        <strong>{cacheRateLabel}</strong>
      </span>
      <i aria-hidden="true" />
      <span>
        <Sigma size={12} />
        <small>{t('chat:focusSession.sessionTokens')}</small>
        <strong>{formatTokenCount(totalTokens)}</strong>
      </span>
    </div>
  )
}

export function ContextUsageIndicator({
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
    ? t('chat:focusSession.autoCompactionThresholdAboutPercent', { percent: currentThreshold })
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

export function SessionModelSelect({
  value,
  models,
  onChange,
  disabled,
  compact = false,
}: {
  value: string
  models: ModelOption[]
  onChange: (model: string) => void
  disabled?: boolean
  compact?: boolean
}) {
  const { t } = useI18n()
  const currentModel = models.find((model) => model.key === value)
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
          : t('chat:focusSession.currentModelModelClickToSwitch', { model: currentLabel })
      }
    >
      <Bot size={compact ? 11 : 14} />
      <AppSelect
        value={value}
        onChange={(event) => onChange(event.target.value)}
        disabled={disabled || models.length === 0}
        aria-label={t('chat:focusSession.currentChatModel')}
      >
        {!currentModel && <option value={value}>{value.split('/').at(-1)}</option>}
        {models.map((model) => (
          <option key={model.key} value={model.key}>
            {model.providerName} · {model.label}
          </option>
        ))}
      </AppSelect>
    </div>
  )
}

export function SessionThinkingSelect({
  value,
  levels,
  status = 'supported',
  message = '',
  onChange,
  disabled,
  compact = false,
}: {
  value: string
  levels: string[]
  status?: string
  message?: string
  onChange: (level: string) => void
  disabled?: boolean
  compact?: boolean
}) {
  const { t } = useI18n()
  const current = value || levels[0] || 'off'
  const loading = !status && levels.length === 0
  const supported = status !== 'unsupported' && levels.length > 0
  const fixed = supported && levels.length <= 1 && levels.includes(current)
  const title = loading
    ? t('chat:focusSession.loadingThinkingLevels')
    : !supported
      ? message || t('chat:focusSession.thinkingLevelUnsupported')
      : fixed
        ? t('chat:focusSession.thinkingLevelFixed', { level: current })
        : disabled
          ? t('chat:focusSession.currentThinkingLevelLevelCannotSwitchWhileRunning', {
              level: current,
            })
          : t('chat:focusSession.currentThinkingLevelLevelClickToSwitch', { level: current })
  return (
    <div
      className={`session-model-select session-thinking-select icon-only ${compact ? 'compact' : ''}`}
      title={title}
    >
      <Brain size={compact ? 11 : 14} />
      <AppSelect
        value={current}
        onChange={(event) => onChange(event.target.value)}
        disabled={disabled || loading || !supported || fixed}
        aria-label={t('chat:focusSession.currentThinkingLevel')}
      >
        {!levels.includes(current) && <option value={current}>{current}</option>}
        {levels.map((level) => (
          <option key={level} value={level}>
            {level}
          </option>
        ))}
      </AppSelect>
    </div>
  )
}

function executionModeOptions(t: Translate): ExecutionModeOption[] {
  return [
    [
      'read-only',
      t('chat:focusSession.readOnly'),
      t('chat:focusSession.onlyInspectAndAnalyzeContent'),
      Eye,
    ],
    [
      'workspace-write',
      t('chat:focusSession.workspaceWrite'),
      t('chat:focusSession.workspaceWriteRunsCommandsWithAutomaticApproval'),
      Pencil,
    ],
    [
      'full-access',
      t('chat:focusSession.fullAccess'),
      t('chat:focusSession.fullAccessRunsShellWithoutPerCommandApproval'),
      ShieldOff,
    ],
  ]
}

export function ExecutionModeSelect({
  value,
  onChange,
  disabled,
  compact = false,
}: {
  value: string
  onChange: (mode: string) => void
  disabled?: boolean
  compact?: boolean
}) {
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
              <strong>{label}</strong>
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
