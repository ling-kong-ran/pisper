// 聚焦视图的运行控制条：停止/继续、上下文用量、模型切换等。
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import {
  BadgeCheck,
  Bot,
  Brain,
  Check,
  Database,
  Eye,
  FileCheck2,
  Gauge,
  ListTodo,
  ShieldOff,
  Sigma,
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { I18nValues } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { formatTokenCount } from '@/lib/format'
import type { EntityRecord, ModelOption, Plan } from '@/types/chat'
import PlanBoard from './PlanBoard'

type Translate = (message: string, values?: I18nValues) => string
type ExecutionModeOption = [string, string, string, LucideIcon]

export function SessionUsageMetrics({
  usage,
  plan,
}: {
  usage?: EntityRecord | null
  plan?: Plan | null
}) {
  const { t } = useI18n()
  const processedTokens = Math.max(0, Number(usage?.processedTokens) || 0)
  const planItems = Array.isArray(plan?.items) ? plan.items : []
  const completedPlanItems = planItems.filter((item) => item?.status === 'completed').length
  const planProgress = planItems.length
    ? t('chat:planBoard.progress', { completed: completedPlanItems, total: planItems.length })
    : ''
  const cacheHitRate = Number(usage?.cacheHitRate)
  const cacheRateKnown = usage?.cacheHitRate != null && Number.isFinite(cacheHitRate)
  const cacheRateLabel = cacheRateKnown ? `${Math.round(Math.max(0, cacheHitRate))}%` : '—'
  const title = t('chat:focusSession.sessionUsageDetail', {
    processed: formatTokenCount(processedTokens),
    input: formatTokenCount(usage?.input),
    output: formatTokenCount(usage?.output),
    cacheRead: formatTokenCount(usage?.cacheRead),
    cacheWrite: formatTokenCount(usage?.cacheWrite),
    reasoning: formatTokenCount(usage?.reasoning),
    requests: Math.max(0, Number(usage?.requests) || 0),
  })

  const metricsTitle = [title, planProgress].filter(Boolean).join('\n')

  return (
    <div
      className="session-usage-metrics [&_>_span]:inline-flex [&_>_span]:min-w-0 [&_>_span]:items-center [&_>_span]:gap-[4px] [&_>_span]:whitespace-nowrap [&_small]:text-inherit [&_small]:text-[inherit] [&_strong]:text-[var(--text-secondary)] [&_strong]:text-[11px] [&_strong]:[font-variant-numeric:tabular-nums] [&_strong]:font-[600] [&_svg]:text-[var(--text-tertiary)] [&_svg]:opacity-[.9] [&_>_i]:w-[1px] [&_>_i]:h-[10px] [&_>_i]:bg-[var(--border-subtle,var(--stroke-soft,#ddd))] @max-[470px]:[&_small]:hidden flex min-w-0 min-h-[26px] items-center justify-start gap-[10px] m-0 text-inherit text-[10px]"
      title={metricsTitle}
      aria-label={metricsTitle}
    >
      <span>
        <Database size={12} />
        <small>{t('chat:focusSession.cacheHitRate')}</small>
        <strong>{cacheRateLabel}</strong>
      </span>
      <i aria-hidden="true" />
      <span>
        <Sigma size={12} />
        <small>{t('chat:focusSession.processedTokens')}</small>
        <strong>{formatTokenCount(processedTokens)}</strong>
      </span>
      {planProgress && (
        <>
          <i aria-hidden="true" />
          <Popover>
            <PopoverTrigger asChild>
              <button
                type="button"
                className="session-plan-progress inline-flex min-w-0 items-center gap-[4px] whitespace-nowrap hover:bg-[var(--surface-hover)] hover:text-[var(--star-strong)] data-[state=open]:bg-[var(--surface-hover)] data-[state=open]:text-[var(--star-strong)] focus-visible:[outline:2px_solid_var(--accent-border)] focus-visible:[outline-offset:1px] min-h-[24px] border-0 rounded-[var(--r-xs)] bg-transparent [padding:2px_5px] text-inherit cursor-pointer"
                title={t('chat:planBoard.openCurrentPlan', { progress: planProgress })}
                aria-label={t('chat:planBoard.openCurrentPlan', { progress: planProgress })}
              >
                <ListTodo size={12} />
                <strong>{planProgress}</strong>
              </button>
            </PopoverTrigger>
            <PopoverContent
              className="session-plan-popover w-[min(440px,calc(100vw_-_24px))] max-h-[min(360px,55dvh)] overflow-auto [padding:6px]"
              align="start"
              side="top"
              sideOffset={7}
            >
              <PlanBoard plan={plan ?? null} />
            </PopoverContent>
          </Popover>
        </>
      )}
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
  const compactionCapacityText =
    usage?.autoCompactEnabled && usage.compactionCapacityPercent != null
      ? t('chat:focusSession.contextUsageToCompactionTokensPercent', {
          percent: Math.round(Number(usage.compactionCapacityPercent)),
          tokens: formatTokenCount(usage.remainingBeforeCompaction),
        })
      : ''
  const label = `${usageText} · ${thresholdText}${compactionCapacityText ? ` · ${compactionCapacityText}` : ''}`
  const tokenLabel = `${usage?.tokens == null ? '—' : formatTokenCount(usage.tokens)} / ${formatTokenCount(contextWindow)}`

  // 保存压缩阈值：本地持久化到 store 并回调通知。
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
          className={`context-usage-chip hover:opacity-100 [&_>_svg]:opacity-[.72] [&_>_span]:inline-flex [&_>_span]:[align-items:baseline] [&_>_span]:gap-[4px] [&_>_span]:whitespace-nowrap [&_>_span_strong]:text-inherit [&_>_span_strong]:text-[11px] [&_>_span_strong]:font-[500] [&_>_span_small]:text-inherit [&_>_span_small]:text-[10px] [&_>_span_small]:font-[600] [&_>_i]:block [&_>_i]:w-[24px] [&_>_i]:h-[2px] [&_>_i]:overflow-hidden [&_>_i]:rounded-[var(--r-pill)] [&_>_i]:bg-[var(--stroke-soft)] [&_>_i_>_b]:block [&_>_i_>_b]:h-full [&_>_i_>_b]:rounded-[inherit] [&_>_i_>_b]:bg-[var(--text-muted)] [&_>_i_>_b]:[transition:width_var(--d2)_var(--ease-out)] [&.warning_>_span_small]:text-[var(--warning-strong)] [&.warning_>_i_>_b]:bg-[var(--warning-strong)] [&.danger_>_span_small]:text-[var(--danger)] [&.danger_>_i_>_b]:bg-[var(--danger)] [&.unknown_>_i_>_b]:!w-[0] @max-[470px]:grid-cols-[auto_auto] @max-[470px]:[&_>_i]:hidden grid h-[24px] flex-none grid-cols-[auto_auto_24px] items-center gap-[4px] border-0 bg-transparent [padding:0_2px] text-[var(--text-tertiary)] text-[10px] opacity-[.78] cursor-pointer [transition:opacity_var(--d1)_var(--ease-out)] ${tone}`}
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
      <PopoverContent
        className="context-usage-popover [&_input[type='range']]:w-full [&_input[type='range']]:h-[20px] [&_input[type='range']]:m-0 [&_input[type='range']]:[accent-color:var(--brand-blue-strong)] [&_input[type='range']]:cursor-pointer [&_>_small]:min-h-[16px] [&_>_small]:text-[var(--text-muted)] [&_>_small]:text-[10px] [&_>_small]:leading-[1.4] [&_>_small.error]:text-[var(--danger)] w-[248px] gap-[8px] [padding:12px]"
        align="end"
        sideOffset={8}
      >
        {compactionCapacityText && (
          <small className="text-[var(--text-muted)]">{compactionCapacityText}</small>
        )}
        <div className="context-threshold-heading flex items-center justify-between [&_output]:text-[var(--brand-blue-strong)] [&_output]:[font-variant-numeric:tabular-nums] text-[var(--text)] text-[12px] font-[600]">
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
        <div
          className="context-threshold-scale flex items-center justify-between text-[var(--text-tertiary)] text-[10px]"
          aria-hidden="true"
        >
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
      className={`session-model-select [&.compact]:max-w-[118px] [&.compact]:h-[32px] [&.compact]:rounded-[var(--r-xs)] [&_[data-slot='select-trigger']]:w-full [&_[data-slot='select-trigger']]:h-full [&_[data-slot='select-trigger']]:min-h-0 [&_[data-slot='select-trigger']]:overflow-hidden [&_[data-slot='select-trigger']]:border-0 [&_[data-slot='select-trigger']]:[outline:0] [&_[data-slot='select-trigger']]:bg-transparent [&_[data-slot='select-trigger']]:p-[0_22px_0_8px] [&_[data-slot='select-trigger']]:text-inherit [&_[data-slot='select-trigger']]:text-[13px] [&_[data-slot='select-trigger']]:font-[600] [&_[data-slot='select-trigger']]:text-ellipsis [&_[data-slot='select-trigger']]:whitespace-nowrap [&.compact_[data-slot='select-trigger']]:text-[12px] [&_>_svg]:absolute [&_>_svg]:right-[6px] [&_>_svg]:flex-none [&_>_svg]:pointer-events-none [&:has([data-slot='select-trigger']:disabled)]:[cursor:not-allowed] [&:has([data-slot='select-trigger']:disabled)]:opacity-[.55] [.focus-composer_&]:w-[38px] [.focus-composer_&]:min-w-[38px] [.focus-composer_&]:h-[38px] [.focus-composer_&]:rounded-[var(--r-sm)] [.focus-session.has-conversation_.focus-composer_&]:w-[36px] [.focus-session.has-conversation_.focus-composer_&]:min-w-[36px] [.focus-session.has-conversation_.focus-composer_&]:h-[36px] dark:bg-[var(--accent-soft)] @max-[700px]:[.focus-composer-footer.tools-open_&]:hidden relative flex max-w-[170px] h-[30px] items-center rounded-[var(--r-xs)] bg-[var(--accent-soft)] text-[var(--star-strong)] icon-only [.session-model-select&]:w-[38px] [.session-model-select&]:min-w-[38px] [.session-model-select&]:max-w-[none] [.session-model-select&]:overflow-hidden [.session-model-select&]:flex-none [.session-model-select&]:justify-center [.session-model-select&]:bg-[var(--surface-muted)] [.session-model-select&]:text-[var(--text-muted)] [.session-model-select&.compact]:w-[32px] [.session-model-select&.compact]:min-w-[32px] [.session-model-select&:hover]:bg-[var(--star-soft)] [.session-model-select&:hover]:text-[var(--star-strong)] [.session-model-select&_>_svg]:[position:static] [.session-model-select&_[data-slot='select-trigger']]:absolute [.session-model-select&_[data-slot='select-trigger']]:inset-0 [.session-model-select&_[data-slot='select-trigger']]:w-full [.session-model-select&_[data-slot='select-trigger']]:h-full [.session-model-select&_[data-slot='select-trigger']]:min-h-0 [.session-model-select&_[data-slot='select-trigger']]:p-0 [.session-model-select&_[data-slot='select-trigger']]:opacity-0 [.session-model-select&_[data-slot='select-trigger']]:cursor-pointer [[data-theme='dark']_.session-model-select&]:bg-[var(--surface-muted)] [.permission-mode-select&]:min-w-0 [.permission-mode-select.compact&]:min-w-0 [.permission-mode-trigger&]:relative [.permission-mode-trigger&]:w-[38px] [.permission-mode-trigger&]:grid-cols-[1fr] [.permission-mode-trigger&]:[justify-items:center] [.permission-mode-trigger&]:p-0 [.permission-mode-select.compact_.permission-mode-trigger&]:w-[32px] [.permission-mode-trigger&.mode-auto]:text-[var(--star-strong)] [.permission-mode-trigger&.mode-ignore]:text-[var(--danger)] [.permission-mode-trigger&.mode-full-access]:text-[var(--danger)] [.focus-session.has-conversation_.focus-composer_.permission-mode-trigger&]:w-[36px] [.focus-session.has-conversation_.focus-composer_.permission-mode-trigger&]:h-[36px] @max-[700px]:[.focus-composer_.permission-mode-trigger&]:w-[36px] max-[650px]:[.focus-composer_.permission-mode-trigger&]:w-[36px] ${compact ? 'compact' : ''}`}
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
      className={`session-model-select [&.compact]:max-w-[118px] [&.compact]:h-[32px] [&.compact]:rounded-[var(--r-xs)] [&_[data-slot='select-trigger']]:w-full [&_[data-slot='select-trigger']]:h-full [&_[data-slot='select-trigger']]:min-h-0 [&_[data-slot='select-trigger']]:overflow-hidden [&_[data-slot='select-trigger']]:border-0 [&_[data-slot='select-trigger']]:[outline:0] [&_[data-slot='select-trigger']]:bg-transparent [&_[data-slot='select-trigger']]:p-[0_22px_0_8px] [&_[data-slot='select-trigger']]:text-inherit [&_[data-slot='select-trigger']]:text-[13px] [&_[data-slot='select-trigger']]:font-[600] [&_[data-slot='select-trigger']]:text-ellipsis [&_[data-slot='select-trigger']]:whitespace-nowrap [&.compact_[data-slot='select-trigger']]:text-[12px] [&_>_svg]:absolute [&_>_svg]:right-[6px] [&_>_svg]:flex-none [&_>_svg]:pointer-events-none [&:has([data-slot='select-trigger']:disabled)]:[cursor:not-allowed] [&:has([data-slot='select-trigger']:disabled)]:opacity-[.55] [.focus-composer_&]:w-[38px] [.focus-composer_&]:min-w-[38px] [.focus-composer_&]:h-[38px] [.focus-composer_&]:rounded-[var(--r-sm)] [.focus-session.has-conversation_.focus-composer_&]:w-[36px] [.focus-session.has-conversation_.focus-composer_&]:min-w-[36px] [.focus-session.has-conversation_.focus-composer_&]:h-[36px] dark:bg-[var(--accent-soft)] @max-[700px]:[.focus-composer-footer.tools-open_&]:hidden relative flex max-w-[170px] h-[30px] items-center rounded-[var(--r-xs)] bg-[var(--accent-soft)] text-[var(--star-strong)] session-thinking-select [.composer-tool-tray_&]:w-[38px] [.composer-tool-tray_&]:min-w-[38px] [.composer-tool-tray_&]:h-[38px] [.composer-tool-tray_&]:flex-none [.focus-composer_&]:flex-none @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:min-w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[700px]:[.composer-tool-tray_&]:p-0 @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:min-w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px] icon-only [.session-model-select&]:w-[38px] [.session-model-select&]:min-w-[38px] [.session-model-select&]:max-w-[none] [.session-model-select&]:overflow-hidden [.session-model-select&]:flex-none [.session-model-select&]:justify-center [.session-model-select&]:bg-[var(--surface-muted)] [.session-model-select&]:text-[var(--text-muted)] [.session-model-select&.compact]:w-[32px] [.session-model-select&.compact]:min-w-[32px] [.session-model-select&:hover]:bg-[var(--star-soft)] [.session-model-select&:hover]:text-[var(--star-strong)] [.session-model-select&_>_svg]:[position:static] [.session-model-select&_[data-slot='select-trigger']]:absolute [.session-model-select&_[data-slot='select-trigger']]:inset-0 [.session-model-select&_[data-slot='select-trigger']]:w-full [.session-model-select&_[data-slot='select-trigger']]:h-full [.session-model-select&_[data-slot='select-trigger']]:min-h-0 [.session-model-select&_[data-slot='select-trigger']]:p-0 [.session-model-select&_[data-slot='select-trigger']]:opacity-0 [.session-model-select&_[data-slot='select-trigger']]:cursor-pointer [[data-theme='dark']_.session-model-select&]:bg-[var(--surface-muted)] [.permission-mode-select&]:min-w-0 [.permission-mode-select.compact&]:min-w-0 [.permission-mode-trigger&]:relative [.permission-mode-trigger&]:w-[38px] [.permission-mode-trigger&]:grid-cols-[1fr] [.permission-mode-trigger&]:[justify-items:center] [.permission-mode-trigger&]:p-0 [.permission-mode-select.compact_.permission-mode-trigger&]:w-[32px] [.permission-mode-trigger&.mode-auto]:text-[var(--star-strong)] [.permission-mode-trigger&.mode-ignore]:text-[var(--danger)] [.permission-mode-trigger&.mode-full-access]:text-[var(--danger)] [.focus-session.has-conversation_.focus-composer_.permission-mode-trigger&]:w-[36px] [.focus-session.has-conversation_.focus-composer_.permission-mode-trigger&]:h-[36px] @max-[700px]:[.focus-composer_.permission-mode-trigger&]:w-[36px] max-[650px]:[.focus-composer_.permission-mode-trigger&]:w-[36px] ${compact ? 'compact' : ''}`}
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
      'approval-required',
      t('chat:focusSession.approvalRequired'),
      t('chat:focusSession.approvalRequiredShowsADiffBeforeWriting'),
      FileCheck2,
    ],
    [
      'workspace-write',
      t('chat:focusSession.workspaceWrite'),
      t('chat:focusSession.workspaceWriteRunsCommandsWithAutomaticApproval'),
      BadgeCheck,
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
        className="permission-mode-menu [&_>_button]:grid [&_>_button]:w-full [&_>_button]:min-h-[48px] [&_>_button]:grid-cols-[auto_minmax(0,1fr)_auto] [&_>_button]:items-center [&_>_button]:gap-[8px] [&_>_button]:border-0 [&_>_button]:rounded-[var(--r-sm)] [&_>_button]:bg-transparent [&_>_button]:text-[var(--text)] [&_>_button]:p-[6px_7px] [&_>_button]:text-left [&_>_button:hover]:bg-[var(--accent-soft)] [&_>_button.active]:bg-[var(--accent-soft)] [&_>_button_>_span:nth-child(2)]:flex [&_>_button_>_span:nth-child(2)]:min-w-0 [&_>_button_>_span:nth-child(2)]:flex-col [&_>_button_>_span:nth-child(2)]:gap-[2px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_small]:leading-[1.4] [&_>_button_>_svg]:text-[var(--star-strong)] max-[650px]:[.focus-composer_&]:right-[auto] max-[650px]:[.focus-composer_&]:left-0 max-[650px]:[.focus-composer_&]:w-[min(250px,calc(100vw_-_76px))] absolute z-[35] right-0 [bottom:calc(100%_+_8px)] w-[250px] overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)] [padding:5px] shadow-[0_18px_42px_-18px_var(--menu-shadow)] execution-mode-menu !fixed !right-auto !bottom-auto z-[80]"
        style={menuPosition}
        role="menu"
      >
        <div className="[padding:6px_8px_4px] text-[var(--text-muted)] text-[11px] font-[700] tracking-[.04em] [text-transform:uppercase]">
          {t('chat:focusSession.executionMode')}
        </div>
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
            <span
              className={`permission-level [&.level-auto]:bg-[var(--accent-soft)] [&.level-auto]:text-[var(--star-strong)] [&.level-ignore]:bg-[var(--danger-soft)] [&.level-ignore]:text-[var(--danger)] [&.level-full-access]:bg-[var(--danger-soft)] [&.level-full-access]:text-[var(--danger)] grid w-[32px] h-[32px] place-items-center rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-[var(--text-muted)] level-${mode}`}
            >
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
        className={`permission-mode-select [&.compact]:min-w-[68px] [&.compact]:h-[32px] [.focus-composer_&]:h-[38px] [.focus-session.has-conversation_.focus-composer_&]:h-[36px] @max-[700px]:[.focus-composer_&]:min-w-0 max-[650px]:[.focus-composer_&]:min-w-0 relative min-w-[78px] h-[32px] text-[var(--text-tertiary)] execution-mode-select icon-only [.session-model-select&]:w-[38px] [.session-model-select&]:min-w-[38px] [.session-model-select&]:max-w-[none] [.session-model-select&]:overflow-hidden [.session-model-select&]:flex-none [.session-model-select&]:justify-center [.session-model-select&]:bg-[var(--surface-muted)] [.session-model-select&]:text-[var(--text-muted)] [.session-model-select&.compact]:w-[32px] [.session-model-select&.compact]:min-w-[32px] [.session-model-select&:hover]:bg-[var(--star-soft)] [.session-model-select&:hover]:text-[var(--star-strong)] [.session-model-select&_>_svg]:[position:static] [.session-model-select&_[data-slot='select-trigger']]:absolute [.session-model-select&_[data-slot='select-trigger']]:inset-0 [.session-model-select&_[data-slot='select-trigger']]:w-full [.session-model-select&_[data-slot='select-trigger']]:h-full [.session-model-select&_[data-slot='select-trigger']]:min-h-0 [.session-model-select&_[data-slot='select-trigger']]:p-0 [.session-model-select&_[data-slot='select-trigger']]:opacity-0 [.session-model-select&_[data-slot='select-trigger']]:cursor-pointer [[data-theme='dark']_.session-model-select&]:bg-[var(--surface-muted)] [.permission-mode-select&]:min-w-0 [.permission-mode-select.compact&]:min-w-0 [.permission-mode-trigger&]:relative [.permission-mode-trigger&]:w-[38px] [.permission-mode-trigger&]:grid-cols-[1fr] [.permission-mode-trigger&]:[justify-items:center] [.permission-mode-trigger&]:p-0 [.permission-mode-select.compact_.permission-mode-trigger&]:w-[32px] [.permission-mode-trigger&.mode-auto]:text-[var(--star-strong)] [.permission-mode-trigger&.mode-ignore]:text-[var(--danger)] [.permission-mode-trigger&.mode-full-access]:text-[var(--danger)] [.focus-session.has-conversation_.focus-composer_.permission-mode-trigger&]:w-[36px] [.focus-session.has-conversation_.focus-composer_.permission-mode-trigger&]:h-[36px] @max-[700px]:[.focus-composer_.permission-mode-trigger&]:w-[36px] max-[650px]:[.focus-composer_.permission-mode-trigger&]:w-[36px] ${compact ? 'compact' : ''}    ${open ? 'open' : ''}`}
      >
        <button
          type="button"
          className={`permission-mode-trigger hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)] hover:text-[var(--star-strong)] [.permission-mode-select.open_&]:border-[var(--accent-border)] [.permission-mode-select.open_&]:bg-[var(--accent-soft)] [.permission-mode-select.open_&]:text-[var(--star-strong)] disabled:[cursor:not-allowed] disabled:opacity-[.55] [.permission-mode-select.compact_&]:gap-[3px] [.permission-mode-select.compact_&]:rounded-[var(--r-xs)] [.permission-mode-select.compact_&]:p-[0_4px] [.permission-mode-select.compact_&]:text-[13px] [.focus-composer_&]:rounded-[var(--r-sm)] dark:hover:bg-[var(--accent-soft)] grid w-full h-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[5px] [border:1px_solid_transparent] rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-inherit [padding:0_7px] text-[12px] font-[700] icon-only [.session-model-select&]:w-[38px] [.session-model-select&]:min-w-[38px] [.session-model-select&]:max-w-[none] [.session-model-select&]:overflow-hidden [.session-model-select&]:flex-none [.session-model-select&]:justify-center [.session-model-select&]:bg-[var(--surface-muted)] [.session-model-select&]:text-[var(--text-muted)] [.session-model-select&.compact]:w-[32px] [.session-model-select&.compact]:min-w-[32px] [.session-model-select&:hover]:bg-[var(--star-soft)] [.session-model-select&:hover]:text-[var(--star-strong)] [.session-model-select&_>_svg]:[position:static] [.session-model-select&_[data-slot='select-trigger']]:absolute [.session-model-select&_[data-slot='select-trigger']]:inset-0 [.session-model-select&_[data-slot='select-trigger']]:w-full [.session-model-select&_[data-slot='select-trigger']]:h-full [.session-model-select&_[data-slot='select-trigger']]:min-h-0 [.session-model-select&_[data-slot='select-trigger']]:p-0 [.session-model-select&_[data-slot='select-trigger']]:opacity-0 [.session-model-select&_[data-slot='select-trigger']]:cursor-pointer [[data-theme='dark']_.session-model-select&]:bg-[var(--surface-muted)] [.permission-mode-select&]:min-w-0 [.permission-mode-select.compact&]:min-w-0 [.permission-mode-trigger&]:relative [.permission-mode-trigger&]:w-[38px] [.permission-mode-trigger&]:grid-cols-[1fr] [.permission-mode-trigger&]:[justify-items:center] [.permission-mode-trigger&]:p-0 [.permission-mode-select.compact_.permission-mode-trigger&]:w-[32px] [.permission-mode-trigger&.mode-auto]:text-[var(--star-strong)] [.permission-mode-trigger&.mode-ignore]:text-[var(--danger)] [.permission-mode-trigger&.mode-full-access]:text-[var(--danger)] [.focus-session.has-conversation_.focus-composer_.permission-mode-trigger&]:w-[36px] [.focus-session.has-conversation_.focus-composer_.permission-mode-trigger&]:h-[36px] @max-[700px]:[.focus-composer_.permission-mode-trigger&]:w-[36px] max-[650px]:[.focus-composer_.permission-mode-trigger&]:w-[36px] mode-${current[0]}`}
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
