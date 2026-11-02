// 输入框的模型芯片（模型 · 思考强度）与弹层里的上下文用量、详细统计。
import { useEffect, useRef, useState } from 'react'
import { ChevronRight, ChevronsUpDown } from 'lucide-react'
import type { I18nValues } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { LevelSlider } from '@/components/app/LevelSlider'
import { PopoverSheet } from '@/components/app/PopoverSheet'
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command'
import { Slider } from '@/components/ui/slider'
import { formatTokenCount } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { EntityRecord, ModelOption } from '@/types/chat'
import { formatRunDuration } from './run-activity'

type Translate = (message: string, values?: I18nValues) => string

// 由模型 key 解析展示名（Provider · 模型），未收录时退化为 key 末段。
// 模型芯片只需要这三项：会话页的 ModelOption 与新任务的 LaunchModelOption 都满足。
type ModelChoice = Pick<ModelOption, 'key' | 'label' | 'providerName'>

function resolveModelLabel(model: string, models: ModelChoice[]) {
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
            <Slider
              min={10}
              max={95}
              step={1}
              value={[draftThreshold]}
              className="h-5 w-full"
              aria-label={t('chat:focusSession.autoCompactionThreshold')}
              valueText={`${draftThreshold}%`}
              disabled={savingThreshold}
              onValueChange={([next]) => scheduleThresholdSave(next)}
              onValueCommit={([next]) => commitThreshold(next)}
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

export function thinkingLevelLabel(t: Translate, level: string) {
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

// 输入框右侧的模型芯片（规格 v2：模型 · 思考强度，弹层里选）：会话页与新任务共用这一个，
// 新任务另外传 thinkingIsDefault / onThinkingReset 表示“沿用模型默认档位”。
// 滑条只列当前后端/模型声明支持的档位。
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
  side = 'top',
  thinkingIsDefault = false,
  onThinkingReset,
  triggerClassName,
}: {
  // 手机会话页：弹层换成底部表单（规格 7.2）。
  sheet?: boolean
  side?: 'top' | 'bottom'
  // 新任务：当前档位只是模型默认（没有显式选过）；onThinkingReset 让显式选择回到默认。
  thinkingIsDefault?: boolean
  onThinkingReset?: () => void
  triggerClassName?: string
  model: string
  models: ModelChoice[]
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
  // 切换思考等级期间锁住模型选择，避免两个请求交错。
  const [saving, setSaving] = useState(false)
  const currentModel = models.find((item) => item.key === model)
  const modelLabel =
    currentModel?.label || model.split('/').at(-1) || t('chat:focusSession.toolbarModel')
  const providerGroups = [
    ...models.reduce((groups, item) => {
      const group = groups.get(item.providerName) || []
      group.push(item)
      return groups.set(item.providerName, group)
    }, new Map<string, ModelChoice[]>()),
  ]
  const supported = status !== 'unsupported' && levels.length > 0
  const effortLabel = supported ? thinkingLevelLabel(t, thinkingLevel || levels[0]) : '—'
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
  const commit = async (level: string) => {
    setSaving(true)
    try {
      await onThinkingChange(level)
    } finally {
      setSaving(false)
    }
  }
  return (
    <PopoverSheet
      sheet={sheet}
      title={t('chat:focusSession.modelAndThinking')}
      align="end"
      side={side}
      sideOffset={8}
      className="model-effort-popover w-[272px] max-w-[calc(100vw_-_24px)] gap-0 p-1"
      trigger={
        <button
          type="button"
          className={cn(
            'model-effort-pill inline-flex h-[30px] min-w-0 max-w-[260px] cursor-pointer items-center gap-1.5 rounded-sm border border-transparent pr-1.5 pl-2.5 text-[13px] text-content transition-colors hover:bg-surface-hover disabled:cursor-default disabled:opacity-60 aria-expanded:bg-surface-hover max-[650px]:h-11 max-[650px]:gap-1 max-[650px]:rounded-md max-[650px]:border-stroke max-[650px]:pr-2 max-[650px]:pl-2.5 max-[650px]:text-sm pointer-coarse:h-11',
            triggerClassName,
          )}
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
      <div className="model-effort-model flex flex-col gap-1 p-1.5 pb-2">
        <span className="px-0.5 text-xs font-medium text-content-muted">
          {t('chat:focusSession.toolbarModel')}
        </span>
        {/* 模型直接列在弹层里（shadcn Command）：按服务商分组、超过 6 个时可搜索、当前模型打勾；
            不再套第二层下拉，长列表只在这一段里滚动，手机上滑动列表也不会误选。 */}
        <Command
          defaultValue={model}
          // 按子串匹配模型名与服务商：cmdk 默认的模糊匹配会把“sol”匹配到 Sonnet。
          filter={(value, search, keywords) =>
            [value, ...(keywords || [])]
              .join(' ')
              .toLowerCase()
              .includes(search.trim().toLowerCase())
              ? 1
              : 0
          }
          className="rounded-sm! border border-stroke-soft bg-transparent p-0"
          aria-label={t('chat:focusSession.currentChatModel')}
        >
          {models.length > 6 && <CommandInput placeholder={t('chat:focusSession.searchModels')} />}
          <CommandList className="max-h-56">
            <CommandEmpty className="py-4 text-xs text-content-muted">
              {models.length
                ? t('chat:focusSession.noMatchingModels')
                : t('chat:focusSession.noModelsAvailable')}
            </CommandEmpty>
            {!currentModel && model && (
              <CommandGroup>
                <CommandItem value={model} data-checked disabled>
                  <span className="min-w-0 truncate">{modelLabel}</span>
                </CommandItem>
              </CommandGroup>
            )}
            {providerGroups.map(([provider, items]) => (
              <CommandGroup key={provider} heading={provider}>
                {items.map((item) => (
                  <CommandItem
                    key={item.key}
                    value={item.key}
                    keywords={[item.label, item.providerName]}
                    data-checked={item.key === model}
                    disabled={modelDisabled || saving}
                    onSelect={() => {
                      if (item.key !== model) onModelChange(item.key)
                    }}
                  >
                    <span className="min-w-0 truncate">{item.label}</span>
                  </CommandItem>
                ))}
              </CommandGroup>
            ))}
          </CommandList>
        </Command>
      </div>
      {supported && (
        <LevelSlider
          className="border-t border-stroke-soft p-1.5 pt-2"
          title={t('chat:focusSession.thinkingLevelTitle')}
          levels={levels}
          value={thinkingLevel}
          labelFor={(level) => thinkingLevelLabel(t, level)}
          onCommit={commit}
          disabled={disabled}
          ariaLabel={t('chat:focusSession.currentThinkingLevel')}
          trailing={
            thinkingIsDefault ? (
              <span className="shrink-0 text-xs text-content-faint">
                {t('chat:focusSession.thinkingModelDefault')}
              </span>
            ) : onThinkingReset ? (
              <button
                type="button"
                className="shrink-0 cursor-pointer rounded-sm text-xs text-content-muted underline-offset-2 hover:text-content hover:underline focus-visible:outline-2 focus-visible:outline-focus"
                onClick={onThinkingReset}
              >
                {t('chat:focusSession.thinkingReset')}
              </button>
            ) : undefined
          }
        />
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
