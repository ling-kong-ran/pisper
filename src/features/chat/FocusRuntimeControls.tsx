// 输入框的模型芯片（模型 · 思考强度）与弹层里的上下文用量、详细统计。
import { useEffect, useRef, useState } from 'react'
import { Bot, Brain, ChevronRight, ChevronsUpDown } from 'lucide-react'
import type { I18nValues } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { PopoverSheet } from '@/components/app/PopoverSheet'
import { formatTokenCount } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { EntityRecord, ModelOption } from '@/types/chat'
import { formatRunDuration } from './run-activity'

type Translate = (message: string, values?: I18nValues) => string

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

// 模型芯片弹层底部的用量段（规格 v2 第 2.5 / 4.3 节）：上下文用量、本会话 tok、缓存命中，
// 以及自动压缩阈值与详细统计。原来页头的上下文环与“+ › 详情”里的读数都收到这里。
// 只写真实拿到的数：上下文窗口未知时整段不显示，缓存命中未上报时写“—”并说明原因。
export function ContextUsageSection({
  usage,
  sessionUsage,
  model,
  availableModels,
  onThresholdChange,
}: {
  usage?: EntityRecord | null
  sessionUsage?: EntityRecord | null
  model?: string
  availableModels?: ModelOption[]
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

  const known = usage?.percent != null && Number.isFinite(Number(usage.percent))
  const percent = known ? Math.max(0, Number(usage?.percent)) : null
  const processedTokens = Math.max(0, Number(sessionUsage?.processedTokens) || 0)
  const cacheHitRate = Number(sessionUsage?.cacheHitRate)
  const cacheRateKnown = sessionUsage?.cacheHitRate != null && Number.isFinite(cacheHitRate)
  const contextText = contextWindow
    ? `${usage?.tokens == null ? '—' : formatTokenCount(usage.tokens)} / ${formatTokenCount(contextWindow)}${percent == null ? '' : ` · ${Math.round(percent)}%`}`
    : ''
  const compactionCapacityText =
    usage?.autoCompactEnabled && usage.compactionCapacityPercent != null
      ? t('chat:focusSession.contextUsageToCompactionTokensPercent', {
          percent: Math.round(Number(usage.compactionCapacityPercent)),
          tokens: formatTokenCount(usage.remainingBeforeCompaction),
        })
      : ''

  // 保存压缩阈值：写回 Runtime；失败时保留草稿值并写出原因。
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
  const row = (label: string, value: string, key: string, title?: string) => (
    <div
      className="flex items-baseline justify-between gap-3 text-xs"
      data-usage-row={key}
      title={title}
    >
      <span className="shrink-0 text-content-muted">{label}</span>
      <strong className="min-w-0 truncate text-right font-medium text-content tabular-nums">
        {value}
      </strong>
    </div>
  )

  return (
    <div
      className="context-usage-section grid gap-1.5 border-t border-stroke-soft px-2 pt-2 pb-1.5"
      data-context-usage
    >
      {contextText && (
        <div className="grid gap-1">
          {row(t('board:composer.contextUsage'), contextText, 'context')}
          <span
            aria-hidden="true"
            className="block h-[3px] overflow-hidden rounded-full bg-surface-active"
          >
            <span
              className="block h-full rounded-full bg-content-secondary"
              style={{ width: `${Math.min(100, percent || 0)}%` }}
            />
          </span>
        </div>
      )}
      {processedTokens > 0 &&
        row(
          t('board:composer.sessionTokens'),
          t('board:composer.tokens', { tokens: formatTokenCount(processedTokens) }),
          'tokens',
        )}
      {row(
        t('board:composer.cacheHitLabel'),
        cacheRateKnown ? `${Math.round(Math.max(0, cacheHitRate))}%` : '—',
        'cache',
        cacheRateKnown ? undefined : t('chat:focusSession.cacheHitRateUnavailable'),
      )}
      {contextWindow > 0 && (
        <details className="group/usage min-w-0 text-xs">
          <summary className="flex min-h-7 cursor-pointer list-none items-center gap-1 rounded-sm text-content-secondary hover:text-content focus-visible:outline-2 focus-visible:outline-focus max-[650px]:min-h-11 [&::-webkit-details-marker]:hidden">
            <ChevronRight
              size={13}
              aria-hidden="true"
              className="transition-transform duration-(--d-fast) group-open/usage:rotate-90"
            />
            {t('chat:focusSession.sessionStats')}
          </summary>
          <div className="grid gap-2 pt-1.5">
            <SessionStatsPanel
              model={model}
              availableModels={availableModels}
              sessionUsage={sessionUsage}
              contextUsage={usage}
            />
            {compactionCapacityText && (
              <small className="text-content-muted">{compactionCapacityText}</small>
            )}
            <div className="flex items-center justify-between font-medium text-content">
              <span>{t('chat:focusSession.autoCompactionThreshold')}</span>
              <output className="tabular-nums">{draftThreshold}%</output>
            </div>
            <input
              type="range"
              min="10"
              max="95"
              step="1"
              value={draftThreshold}
              className="m-0 h-5 w-full cursor-pointer [accent-color:var(--star)]"
              aria-label={t('chat:focusSession.autoCompactionThreshold')}
              disabled={savingThreshold}
              onChange={(event) => scheduleThresholdSave(Number(event.currentTarget.value))}
              onBlur={(event) => commitThreshold(Number(event.currentTarget.value))}
              onPointerUp={(event) => commitThreshold(Number(event.currentTarget.value))}
              onKeyUp={(event) => {
                if (
                  ['ArrowLeft', 'ArrowRight', 'Home', 'End', 'PageUp', 'PageDown'].includes(
                    event.key,
                  )
                )
                  commitThreshold(Number(event.currentTarget.value))
              }}
            />
            <small className={thresholdError ? 'text-danger' : 'text-content-muted'}>
              {thresholdError ||
                (savingThreshold ? t('chat:focusSession.savingCompactionThreshold') : '\u00a0')}
            </small>
          </div>
        </details>
      )}
    </div>
  )
}

// 带文字的选择器按字段处理焦点：描边换成焦点色并加 3px 焦点环（透明原生 select 覆盖整块）。
const LABEL_SELECT_CLASSES =
  'relative flex h-10 min-w-0 items-center gap-1 rounded-sm border border-transparent px-1.5 text-[13px] text-content-secondary hover:bg-surface-hover hover:text-content has-[:focus-visible]:border-focus has-[:focus-visible]:ring-3 has-[:focus-visible]:ring-focus-ring'

const ICON_SELECT_CLASSES =
  'session-model-select relative grid size-[30px] min-w-[30px] place-items-center rounded-sm text-content-muted hover:bg-surface-hover hover:text-content focus-within:ring-2 focus-within:ring-focus max-[650px]:size-11 max-[650px]:min-w-11 [&.compact]:size-8 [&.compact]:min-w-8'

export function SessionModelSelect({
  value,
  models,
  onChange,
  disabled,
  compact = false,
  showLabel = false,
  modelLabelOnly = false,
  displayLabel,
  className,
}: {
  value: string
  models: ModelOption[]
  onChange: (model: string) => void
  disabled?: boolean
  compact?: boolean
  showLabel?: boolean
  modelLabelOnly?: boolean
  displayLabel?: React.ReactNode
  className?: string
}) {
  const { t } = useI18n()
  const currentModel = models.find((model) => model.key === value)
  const currentLabel = currentModel
    ? `${currentModel.providerName} · ${currentModel.label}`
    : value.split('/').at(-1)
  return (
    <div
      className={cn(
        showLabel ? LABEL_SELECT_CLASSES : ICON_SELECT_CLASSES,
        compact && 'compact',
        className,
      )}
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
          {displayLabel ??
            ((modelLabelOnly ? currentModel?.label || value.split('/').at(-1) : currentLabel) ||
              t('chat:focusSession.toolbarModel'))}
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
    max: t('chat:focusSession.thinkingLabel.max'),
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
  deferred = false,
  pendingModel = false,
  usage,
  sheet = false,
}: {
  // 手机会话页：弹层换成底部表单（规格 7.2）。
  sheet?: boolean
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
  deferred?: boolean
  pendingModel?: boolean
  // 弹层底部的用量段（ContextUsageSection）：上下文环从页头移到这里（规格 v2 第 2.5 节）。
  usage?: React.ReactNode
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
  const effortHint = pendingModel
    ? t('chat:focusSession.thinkingAfterModelSwitch')
    : supported
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
    <PopoverSheet
      sheet={sheet}
      title={t('chat:focusSession.modelAndThinking')}
      align="end"
      side="top"
      sideOffset={8}
      className="model-effort-popover w-[272px] max-w-[calc(100vw_-_24px)] gap-0 p-1"
      trigger={
        <button
          type="button"
          className="model-effort-pill inline-flex h-[30px] min-w-0 max-w-[260px] cursor-pointer items-center gap-1.5 rounded-sm border border-transparent pr-1.5 pl-2.5 text-[13px] text-content transition-colors hover:bg-surface-hover aria-expanded:bg-surface-hover max-[650px]:h-11 max-[650px]:gap-1 max-[650px]:rounded-md max-[650px]:border-stroke max-[650px]:pr-2 max-[650px]:pl-2.5 max-[650px]:text-sm pointer-coarse:h-11"
          aria-label={label}
          title={label}
        >
          <span className="min-w-0 truncate font-medium">{modelLabel}</span>
          <span aria-hidden="true" className="text-content-faint">
            ·
          </span>
          <span className="shrink-0 text-content-muted">
            {supported ? thinkingLevelLabel(t, thinkingLevel || levels[0]) : '—'}
          </span>
          <ChevronsUpDown size={14} className="shrink-0 text-content-muted" aria-hidden="true" />
        </button>
      }
    >
      {/* 上段选模型、下段调思考等级；分区标题让两个控件的作用一眼可辨。
            模型行带 1px 描边，而透明选择器只铺满内边距盒；行高取 52px，
            保证有无服务商副标题时可点区域都不低于 48px。 */}
      <div className="model-effort-model flex flex-col gap-1.5 p-1.5 pb-2">
        <span className="px-0.5 text-xs font-medium text-content-muted">
          {t('chat:focusSession.toolbarModel')}
        </span>
        <SessionModelSelect
          showLabel
          value={model}
          models={models}
          onChange={onModelChange}
          disabled={modelDisabled || saving}
          className="h-auto min-h-13 w-full border-control bg-panel px-2 py-1.5 text-content [&>span]:flex-1"
          displayLabel={
            <span className="flex min-w-0 items-center gap-2">
              <span className="grid size-7 shrink-0 place-items-center rounded-sm border border-key-border bg-key text-content-muted shadow-key">
                <Bot size={15} aria-hidden="true" />
              </span>
              <span className="flex min-w-0 flex-1 flex-col text-left">
                <span className="truncate text-[13px] leading-5 font-medium">{modelLabel}</span>
                {currentModel?.providerName && (
                  <span className="truncate text-xs leading-4 text-content-muted">
                    {currentModel.providerName}
                  </span>
                )}
              </span>
              <ChevronsUpDown
                size={14}
                className="shrink-0 text-content-muted"
                aria-hidden="true"
              />
            </span>
          }
        />
      </div>
      {supported && (
        <div className="border-t border-stroke-soft p-1.5 pt-2">
          <div className="flex items-baseline justify-between px-0.5 pb-1">
            <span className="text-xs font-medium text-content-muted">
              {t('chat:focusSession.thinkingLevelTitle')}
            </span>
            <span
              className="model-effort-heading text-[13px] leading-5 font-medium text-accent-fg"
              aria-live="polite"
            >
              {effortLabel}
            </span>
          </div>
          {/* 仪表式刻度：4px 轨道 + 每档一根刻度线；轨道两端对齐滑块圆心（滑块 24px）。 */}
          <div
            className="relative h-7"
            style={
              {
                '--effort-fill': `${levels.length > 1 ? (selected / (levels.length - 1)) * 100 : 0}%`,
              } as React.CSSProperties
            }
          >
            <div aria-hidden="true" className="effort-track absolute inset-x-3 inset-y-0">
              <div className="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 overflow-hidden rounded-full bg-surface-active">
                <div className="h-full bg-star" style={{ width: 'var(--effort-fill)' }} />
              </div>
              <div className="absolute inset-0 flex items-center justify-between">
                {levels.map((level) => (
                  <i key={level} className="h-2.5 w-px bg-stroke-strong" />
                ))}
              </div>
            </div>
            <input
              className="effort-slider relative m-0 h-7 w-full cursor-pointer appearance-none bg-transparent disabled:cursor-default disabled:opacity-60"
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
          {levels.length > 1 && (
            <div
              className="flex justify-between px-0.5 pt-1 text-xs text-content-muted"
              aria-hidden="true"
            >
              <span>{thinkingLevelLabel(t, levels[0])}</span>
              <span>{thinkingLevelLabel(t, levels.at(-1) || '')}</span>
            </div>
          )}
        </div>
      )}
      {deferred && (
        <p className="px-2 pt-1 pb-2 text-xs leading-4 text-content-muted" role="status">
          {t('chat:focusSession.runtimeSelectionDeferred')}
        </p>
      )}
      {(!supported || levels.length === 1) && (
        <p
          className="border-t border-stroke-soft px-2 pt-2 pb-1.5 text-xs leading-relaxed text-content-muted"
          role="status"
        >
          {supported
            ? t('chat:focusSession.thinkingLevelFixed', { level: effortLabel })
            : effortHint}
        </p>
      )}
      {usage}
    </PopoverSheet>
  )
}
