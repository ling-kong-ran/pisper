// 聚焦视图的运行控制条：停止/继续、上下文用量、模型切换等。
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import {
  BadgeCheck,
  Bot,
  Brain,
  Check,
  ChevronRight,
  Database,
  FileCheck2,
  Gauge,
  ListTodo,
  ShieldAlert,
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
import { formatRunDuration } from './run-activity'

type Translate = (message: string, values?: I18nValues) => string
type ExecutionModeOption = [string, string, string, LucideIcon]

// 由模型 key 解析展示名（Provider · 模型），未收录时退化为 key 末段。
function resolveModelLabel(model: string, models: ModelOption[]) {
  const current = models.find((item) => item.key === model)
  return current ? `${current.providerName} · ${current.label}` : model.split('/').at(-1) || model
}

// 详细数据统计面板：在上下文用量弹窗中展示当前模型、token 用量与请求时序
// （首字平均耗时/平均响应时长）。timing 由 runtime 随 sessionUsage 上报并持久化。
export function SessionStatsPanel({
  model,
  availableModels,
  sessionUsage,
  contextUsage,
}: {
  model?: string
  availableModels?: ModelOption[]
  sessionUsage?: EntityRecord | null
  contextUsage?: EntityRecord | null
}) {
  const { t, language } = useI18n()
  const timing =
    sessionUsage?.timing && typeof sessionUsage.timing === 'object' ? sessionUsage.timing : null
  const firstTokenSamples = Math.max(0, Number(timing?.firstTokenSamples) || 0)
  const recordedRequests = Math.max(0, Number(timing?.requests) || 0)
  const avgFirstToken =
    firstTokenSamples > 0 ? Number(timing?.firstTokenTotalMs) / firstTokenSamples : null
  const avgDuration =
    recordedRequests > 0 ? Number(timing?.durationTotalMs) / recordedRequests : null
  const contextTokens =
    contextUsage?.tokens == null ? null : Math.max(0, Number(contextUsage.tokens) || 0)
  // 缓存统计与状态栏同语义：上游从未回传过非零缓存用量时不可知，
  // 显示「—」而不是把缺失当成 0（见 stream-projection 的 cacheReported）。
  const cacheKnown = Boolean(sessionUsage?.cacheReported)
  const rows: Array<[string, string]> = [
    [
      t('chat:focusSession.statCurrentModel'),
      model ? resolveModelLabel(model, availableModels || []) : '—',
    ],
    [
      t('chat:focusSession.statContextTokens'),
      contextTokens == null
        ? '—'
        : `${formatTokenCount(contextTokens)} / ${formatTokenCount(contextUsage?.contextWindow)}`,
    ],
    [t('chat:focusSession.statInputTokens'), formatTokenCount(sessionUsage?.input)],
    [t('chat:focusSession.statOutputTokens'), formatTokenCount(sessionUsage?.output)],
    [
      t('chat:focusSession.statCacheReadTokens'),
      cacheKnown ? formatTokenCount(sessionUsage?.cacheRead) : '—',
    ],
    [
      t('chat:focusSession.statCacheWriteTokens'),
      cacheKnown ? formatTokenCount(sessionUsage?.cacheWrite) : '—',
    ],
    [t('chat:focusSession.statReasoningTokens'), formatTokenCount(sessionUsage?.reasoning)],
    [t('chat:focusSession.statProcessedTokens'), formatTokenCount(sessionUsage?.processedTokens)],
    [t('chat:focusSession.statRequests'), String(Math.max(0, Number(sessionUsage?.requests) || 0))],
    [
      t('chat:focusSession.statAvgFirstToken'),
      avgFirstToken == null ? '—' : formatRunDuration(avgFirstToken, language),
    ],
    [
      t('chat:focusSession.statAvgDuration'),
      avgDuration == null ? '—' : formatRunDuration(avgDuration, language),
    ],
  ]
  return (
    <div
      className="session-stats-panel grid gap-[3px]"
      aria-label={t('chat:focusSession.sessionStats')}
    >
      <div className="text-[var(--text)] text-[length:var(--app-font-size)] font-semibold">
        {t('chat:focusSession.sessionStats')}
      </div>
      {rows.map(([label, value]) => (
        <div
          key={label}
          className="flex items-baseline justify-between gap-[10px] text-[length:var(--app-small-size)]"
        >
          <span className="min-w-0 shrink-0 text-[var(--text-muted)]">{label}</span>
          <strong className="min-w-0 truncate text-right font-[500] text-[var(--text)] [font-variant-numeric:tabular-nums]">
            {value}
          </strong>
        </div>
      ))}
      <small className="text-[var(--text-secondary)] text-[length:var(--app-small-size)] leading-normal">
        {t('chat:focusSession.sessionStatsHint')}
      </small>
    </div>
  )
}

export function SessionUsageMetrics({
  usage,
  plan,
  compact = false,
}: {
  usage?: EntityRecord | null
  plan?: Plan | null
  compact?: boolean
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
  // 上游（常见于第三方中转）未回传缓存用量字段时命中率不可知，
  // 需要单独说明，否则「—」会被误认为界面出错。
  const cacheRateHint = cacheRateKnown ? '' : t('chat:focusSession.cacheHitRateUnavailable')
  const title = t('chat:focusSession.sessionUsageDetail', {
    processed: formatTokenCount(processedTokens),
    input: formatTokenCount(usage?.input),
    output: formatTokenCount(usage?.output),
    cacheRead: formatTokenCount(usage?.cacheRead),
    cacheWrite: formatTokenCount(usage?.cacheWrite),
    reasoning: formatTokenCount(usage?.reasoning),
    requests: Math.max(0, Number(usage?.requests) || 0),
  })

  const metricsTitle = [title, cacheRateHint, planProgress].filter(Boolean).join('\n')

  return (
    <div
      className={`session-usage-metrics [&_>_span]:inline-flex [&_>_span]:min-w-0 [&_>_span]:items-center [&_>_span]:gap-[4px] [&_>_span]:whitespace-nowrap [&_small]:text-inherit [&_small]:font-normal [&_strong]:text-[var(--text-secondary)] [&_strong]:text-[length:var(--app-small-size)] [&_strong]:[font-variant-numeric:tabular-nums] [&_strong]:font-medium [&_svg]:text-[var(--text-tertiary)] [&_svg]:opacity-[.9] [&_>_i]:w-[1px] [&_>_i]:h-[10px] [&_>_i]:bg-[var(--border-subtle,var(--stroke-soft,#ddd))] @max-[470px]:[&_small]:hidden flex min-w-0 min-h-[26px] items-center justify-start gap-[10px] m-0 text-[var(--text-secondary)] text-[length:var(--app-small-size)] ${compact ? '[&_small]:hidden gap-[7px]' : ''}`}
      title={metricsTitle}
      aria-label={metricsTitle}
    >
      {!compact && (
        <>
          <span>
            <Database size={12} />
            <small>{t('chat:focusSession.cacheHitRate')}</small>
            <strong>{cacheRateLabel}</strong>
          </span>
          <i aria-hidden="true" />
        </>
      )}
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
  sessionUsage,
  model,
  availableModels,
  onThresholdChange,
  compact = false,
}: {
  usage?: EntityRecord | null
  sessionUsage?: EntityRecord | null
  model?: string
  availableModels?: ModelOption[]
  onThresholdChange?: (thresholdPercent: number) => Promise<void> | void
  compact?: boolean
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
  const warningAt = Math.max(10, currentThreshold - 15)
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
    const next = Math.min(95, Math.max(10, Math.round(value)))
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
          className={`context-usage-chip hover:opacity-100 [&_>_svg]:opacity-[.72] [&_>_span]:inline-flex [&_>_span]:[align-items:baseline] [&_>_span]:gap-[4px] [&_>_span]:whitespace-nowrap [&_>_span_strong]:text-inherit [&_>_span_strong]:text-[length:var(--app-small-size)] [&_>_span_strong]:font-medium [&_>_span_small]:text-inherit [&_>_span_small]:font-medium [&_>_i]:block [&_>_i]:w-[24px] [&_>_i]:h-[2px] [&_>_i]:overflow-hidden [&_>_i]:rounded-[var(--r-pill)] [&_>_i]:bg-[var(--stroke-soft)] [&_>_i_>_b]:block [&_>_i_>_b]:h-full [&_>_i_>_b]:rounded-[inherit] [&_>_i_>_b]:bg-[var(--text-muted)] [&_>_i_>_b]:[transition:width_var(--d2)_var(--ease-out)] [&.warning_>_span_small]:text-[var(--warning-strong)] [&.warning_>_i_>_b]:bg-[var(--warning-strong)] [&.danger_>_span_small]:text-[var(--danger)] [&.danger_>_i_>_b]:bg-[var(--danger)] [&.unknown_>_i_>_b]:!w-[0] @max-[470px]:grid-cols-[auto_auto] @max-[470px]:[&_>_i]:hidden grid h-[24px] flex-none grid-cols-[auto_auto_24px] items-center gap-[4px] border-0 bg-transparent [padding:0_2px] text-[var(--text-secondary)] text-[length:var(--app-small-size)] cursor-pointer [transition:opacity_var(--d1)_var(--ease-out)] ${compact ? '!h-11 grid-cols-[auto_auto] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] px-2 opacity-100 [&.warning_>_span_strong]:text-[var(--warning-strong)] [&.danger_>_span_strong]:text-[var(--danger)]' : ''} ${tone}`}
          aria-label={label}
          title={label}
        >
          <Gauge size={12} />
          <span>
            <strong>
              {compact ? (roundedPercent == null ? '—' : `${roundedPercent}%`) : tokenLabel}
            </strong>
            {!compact && <small>{roundedPercent == null ? '—' : `${roundedPercent}%`}</small>}
          </span>
          {!compact && (
            <i aria-hidden="true">
              <b style={{ width: `${Math.min(100, percent || 0)}%` }} />
            </i>
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="context-usage-popover [&_input[type='range']]:w-full [&_input[type='range']]:h-[20px] [&_input[type='range']]:m-0 [&_input[type='range']]:[accent-color:var(--brand-blue-strong)] [&_input[type='range']]:cursor-pointer [&_>_small]:min-h-[16px] [&_>_small]:text-[var(--text-secondary)] [&_>_small]:text-[length:var(--app-small-size)] [&_>_small]:leading-normal [&_>_small.error]:text-[var(--danger)] w-[272px] gap-[8px] [padding:12px]"
        align="end"
        sideOffset={8}
      >
        <SessionStatsPanel
          model={model}
          availableModels={availableModels}
          sessionUsage={sessionUsage}
          contextUsage={usage}
        />
        <div role="separator" className="h-px w-full bg-[var(--stroke-soft)]" />
        {compactionCapacityText && (
          <small className="text-[var(--text-muted)]">{compactionCapacityText}</small>
        )}
        <div className="context-threshold-heading flex items-center justify-between [&_output]:text-[var(--brand-blue-strong)] [&_output]:[font-variant-numeric:tabular-nums] text-[var(--text)] text-[length:var(--app-small-size)] font-medium">
          <span>{t('chat:focusSession.autoCompactionThreshold')}</span>
          <output>{draftThreshold}%</output>
        </div>
        <input
          type="range"
          min="10"
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
          className="context-threshold-scale flex items-center justify-between text-[var(--text-secondary)] text-[length:var(--app-small-size)]"
          aria-hidden="true"
        >
          <span>10%</span>
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

const LABEL_SELECT_CLASSES =
  'relative flex h-10 min-w-0 items-center gap-1 rounded-lg px-1.5 text-[13px] text-muted-foreground hover:bg-foreground/5 hover:text-foreground focus-within:ring-2 focus-within:ring-ring'

const ICON_SELECT_CLASSES =
  'session-model-select relative grid size-[38px] min-w-[38px] place-items-center rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-[var(--text-muted)] hover:bg-[var(--star-soft)] hover:text-[var(--star-strong)] focus-within:ring-2 focus-within:ring-[var(--focus-ring)] [&.compact]:size-8 [&.compact]:min-w-8'

export function SessionModelSelect({
  value,
  models,
  onChange,
  disabled,
  compact = false,
  showLabel = false,
  modelLabelOnly = false,
}: {
  value: string
  models: ModelOption[]
  onChange: (model: string) => void
  disabled?: boolean
  compact?: boolean
  showLabel?: boolean
  modelLabelOnly?: boolean
}) {
  const { t } = useI18n()
  const currentModel = models.find((model) => model.key === value)
  const currentLabel = currentModel
    ? `${currentModel.providerName} · ${currentModel.label}`
    : value.split('/').at(-1)
  return (
    <div
      className={`${showLabel ? LABEL_SELECT_CLASSES : ICON_SELECT_CLASSES} ${compact ? 'compact' : ''}`}
      title={
        disabled
          ? t('chat:focusSession.currentModelModelCannotSwitchWhileRunning', {
              model: currentLabel,
            })
          : t('chat:focusSession.currentModelModelClickToSwitch', { model: currentLabel })
      }
    >
      {showLabel ? (
        <span className="min-w-0 truncate">
          {(modelLabelOnly ? currentModel?.label || value.split('/').at(-1) : currentLabel) ||
            t('chat:focusSession.toolbarModel')}
        </span>
      ) : (
        <Bot size={compact ? 11 : 14} />
      )}
      <AppSelect
        className="absolute inset-0 !size-full cursor-pointer !opacity-0"
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

function thinkingLevelLabel(t: Translate, level: string) {
  const thinkingLabels: Record<string, string> = {
    off: t('chat:focusSession.thinkingLabel.off'),
    minimal: t('chat:focusSession.thinkingLabel.minimal'),
    low: t('chat:focusSession.thinkingLabel.low'),
    medium: t('chat:focusSession.thinkingLabel.medium'),
    high: t('chat:focusSession.thinkingLabel.high'),
    xhigh: t('chat:focusSession.thinkingLabel.xhigh'),
  }
  return thinkingLabels[level] || level
}

export function SessionThinkingSelect({
  value,
  levels,
  status = 'supported',
  message = '',
  onChange,
  disabled,
  compact = false,
  showLabel = false,
}: {
  value: string
  levels: string[]
  status?: string
  message?: string
  onChange: (level: string) => void
  disabled?: boolean
  compact?: boolean
  showLabel?: boolean
}) {
  const { t } = useI18n()
  const current = value || levels[0] || 'off'
  const levelLabel = (level: string) => thinkingLevelLabel(t, level)
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
      className={`${showLabel ? LABEL_SELECT_CLASSES : ICON_SELECT_CLASSES} session-thinking-select ${compact ? 'compact' : ''}`}
      title={title}
    >
      {showLabel ? (
        <span className="min-w-0 truncate">{levelLabel(current)}</span>
      ) : (
        <Brain size={compact ? 11 : 14} />
      )}
      <AppSelect
        className="absolute inset-0 !size-full cursor-pointer !opacity-0"
        value={current}
        onChange={(event) => onChange(event.target.value)}
        disabled={disabled || loading || !supported || fixed}
        aria-label={t('chat:focusSession.currentThinkingLevel')}
      >
        {!levels.includes(current) && <option value={current}>{levelLabel(current)}</option>}
        {levels.map((level) => (
          <option key={level} value={level}>
            {levelLabel(level)}
          </option>
        ))}
      </AppSelect>
    </div>
  )
}

// The slider uses only the levels advertised by the current backend/model.
export function ModelThinkingControl({
  model,
  models,
  onModelChange,
  thinkingLevel,
  levels,
  status,
  message,
  onThinkingChange,
  modelDisabled,
  thinkingDisabled,
}: {
  model: string
  models: ModelOption[]
  onModelChange: (model: string) => void
  thinkingLevel: string
  levels: string[]
  status?: string
  message?: string
  onThinkingChange: (level: string) => Promise<void> | void
  modelDisabled?: boolean
  thinkingDisabled?: boolean
}) {
  const { t } = useI18n()
  const [draft, setDraft] = useState<number | null>(null)
  const [saving, setSaving] = useState(false)
  const savingRef = useRef(false)
  const currentModel = models.find((item) => item.key === model)
  const modelLabel =
    currentModel?.label || model.split('/').at(-1) || t('chat:focusSession.toolbarModel')
  const supported = status !== 'unsupported' && levels.length > 0
  const index = Math.max(0, levels.indexOf(thinkingLevel))
  const selected = draft ?? index
  const effortLabel = supported ? thinkingLevelLabel(t, levels[selected] || thinkingLevel) : '—'
  const effortHint = supported
    ? effortLabel
    : message ||
      (status === 'unsupported'
        ? t('chat:focusSession.thinkingLevelUnsupported')
        : t('chat:focusSession.loadingThinkingLevels'))
  const disabled = thinkingDisabled || saving || !supported || levels.length < 2
  const label = `${t('chat:focusSession.modelAndThinking')} · ${resolveModelLabel(model, models)} · ${effortHint}`
  const levelsKey = levels.join('|')
  useEffect(() => {
    setDraft(null)
  }, [model, thinkingLevel, levelsKey])
  const commit = async (nextIndex: number) => {
    const level = levels[nextIndex]
    if (disabled || savingRef.current || !level || level === thinkingLevel) return
    savingRef.current = true
    setSaving(true)
    try {
      await onThinkingChange(level)
    } finally {
      savingRef.current = false
      setSaving(false)
      setDraft(null)
    }
  }
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="model-effort-pill inline-flex h-8 min-w-0 max-w-[220px] items-center gap-1.5 rounded-full bg-foreground/5 px-2.5 text-xs text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={label}
          title={label}
        >
          <span className="min-w-0 truncate">{modelLabel}</span>
          <span className="shrink-0">
            {supported ? thinkingLevelLabel(t, thinkingLevel || levels[0]) : '—'}
          </span>
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        side="top"
        sideOffset={8}
        className="model-effort-popover w-[272px] max-w-[calc(100vw_-_24px)] rounded-[22px] border-border/70 bg-popover p-4 shadow-xl"
      >
        <div className="flex flex-col items-center gap-1 pb-4">
          <output
            className="inline-flex items-center gap-1 text-lg font-semibold text-[#329bff]"
            aria-live="polite"
          >
            {effortLabel}
            <ChevronRight size={17} aria-hidden="true" />
          </output>
          <div className="model-effort-model max-w-full [&>div]:h-7 [&>div]:text-sm">
            <SessionModelSelect
              showLabel
              modelLabelOnly
              value={model}
              models={models}
              onChange={onModelChange}
              disabled={modelDisabled || saving}
            />
          </div>
        </div>
        {supported && (
          <div
            className="relative h-9"
            style={
              {
                '--effort-fill': `${levels.length > 1 ? (selected / (levels.length - 1)) * 100 : 0}%`,
              } as React.CSSProperties
            }
          >
            <div
              aria-hidden="true"
              className="effort-track absolute inset-y-1 inset-x-0 overflow-hidden rounded-full bg-foreground/10"
            >
              <div className="h-full bg-[#329bff]" style={{ width: 'var(--effort-fill)' }} />
              <div className="absolute inset-0 flex items-center justify-between px-3.5">
                {levels.map((level) => (
                  <i key={level} className="size-1.5 rounded-full bg-foreground/20" />
                ))}
              </div>
            </div>
            <input
              className="effort-slider relative m-0 h-9 w-full cursor-pointer appearance-none bg-transparent disabled:cursor-default disabled:opacity-60"
              type="range"
              min={0}
              max={Math.max(1, levels.length - 1)}
              step={1}
              value={selected}
              disabled={disabled}
              aria-label={t('chat:focusSession.currentThinkingLevel')}
              aria-valuetext={effortLabel}
              onChange={(event) => setDraft(Number(event.currentTarget.value))}
              onPointerUp={(event) => void commit(Number(event.currentTarget.value))}
              onBlur={(event) => void commit(Number(event.currentTarget.value))}
              onKeyUp={(event) => {
                if (
                  [
                    'ArrowLeft',
                    'ArrowRight',
                    'ArrowUp',
                    'ArrowDown',
                    'Home',
                    'End',
                    'PageUp',
                    'PageDown',
                  ].includes(event.key)
                )
                  void commit(Number(event.currentTarget.value))
              }}
            />
          </div>
        )}
        {(!supported || levels.length === 1) && (
          <p
            className="pt-2 text-center text-xs leading-relaxed text-muted-foreground"
            role="status"
          >
            {supported
              ? t('chat:focusSession.thinkingLevelFixed', { level: effortLabel })
              : effortHint}
          </p>
        )}
      </PopoverContent>
    </Popover>
  )
}

function executionModeOptions(t: Translate): ExecutionModeOption[] {
  return [
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
      ShieldAlert,
    ],
  ]
}

export function ApprovalModeSelect({
  value,
  onChange,
  disabled,
  compact = false,
  showLabel = false,
}: {
  value: string
  onChange: (mode: string) => void
  disabled?: boolean
  compact?: boolean
  showLabel?: boolean
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [menuPosition, setMenuPosition] = useState({ left: 0, top: 0, width: 270 })
  const rootRef = useRef<HTMLDivElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const options = executionModeOptions(t)
  const current = options.find((item) => item[0] === value) || options[0]
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
        className="permission-mode-menu [&_>_button]:grid [&_>_button]:w-full [&_>_button]:min-h-[48px] [&_>_button]:grid-cols-[auto_minmax(0,1fr)_auto] [&_>_button]:items-center [&_>_button]:gap-[8px] [&_>_button]:border-0 [&_>_button]:rounded-[var(--r-sm)] [&_>_button]:bg-transparent [&_>_button]:text-[var(--text)] [&_>_button]:p-[6px_7px] [&_>_button]:text-left [&_>_button:hover]:bg-[var(--accent-soft)] [&_>_button.active]:bg-[var(--accent-soft)] [&_>_button_>_span:nth-child(2)]:flex [&_>_button_>_span:nth-child(2)]:min-w-0 [&_>_button_>_span:nth-child(2)]:flex-col [&_>_button_>_span:nth-child(2)]:gap-[2px] [&_strong]:text-[length:var(--app-font-size)] [&_strong]:font-medium [&_small]:text-[var(--text-secondary)] [&_small]:font-normal [&_small]:text-[length:var(--app-small-size)] [&_small]:leading-normal [&_>_button_>_svg]:text-[var(--star-strong)] absolute z-[35] right-0 [bottom:calc(100%_+_8px)] w-[250px] overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)] [padding:5px] shadow-[0_18px_42px_-18px_var(--menu-shadow)] execution-mode-menu !fixed !right-auto !bottom-auto z-[80]"
        style={menuPosition}
        role="menu"
      >
        <div className="[padding:6px_8px_4px] text-[var(--text-secondary)] text-[length:var(--app-small-size)] font-semibold">
          {t('chat:focusSession.approvalMode')}
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
        className={`permission-mode-select relative min-w-0 shrink-0 ${compact ? 'compact' : ''} ${open ? 'open' : ''}`}
      >
        <button
          type="button"
          className={`permission-mode-trigger inline-flex h-10 min-w-0 items-center justify-center gap-1.5 rounded-lg bg-transparent px-1.5 text-[13px] transition-colors hover:bg-foreground/5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-55 ${showLabel ? 'max-w-[104px] min-[651px]:max-w-[124px]' : 'w-[38px]'} ${current[0] === 'full-access' ? 'text-orange-600 dark:text-orange-400' : 'text-muted-foreground'} mode-${current[0]}`}
          title={t('chat:focusSession.approvalModeModeDescription', {
            mode: current[1],
            description: current[2],
          })}
          disabled={disabled}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={t('chat:focusSession.approvalModeMode', { mode: current[1] })}
          onClick={() => setOpen((visible) => !visible)}
        >
          <CurrentIcon
            className={`shrink-0 ${showLabel ? 'max-[650px]:hidden' : ''}`}
            size={showLabel ? 16 : compact ? 11 : 14}
          />
          {showLabel && <span className="truncate">{current[1]}</span>}
        </button>
      </div>
      {menu}
    </>
  )
}
