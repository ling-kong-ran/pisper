// 上下文面板的两条条：细条看还剩多少空间，彩条看已用的部分被什么占了。
// 彩条按已用量铺满，不按整个窗口算：用量只有几个百分点时（大窗口模型的常态），按窗口算各段都挤成一个点。
// 颜色跟着分类走，取自主题的 --viz-*，经 dataviz 验证脚本检查过色盲可分和对比度；图例写具体数字，颜色不单独传达信息。
import { useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { formatTokenCount } from '@/lib/format'
import { cn } from '@/lib/utils'
import {
  contextShareLabel,
  type ContextBreakdown,
  type ContextCategoryId,
} from './context-breakdown'

const CATEGORY_COLORS: Record<ContextCategoryId, string> = {
  system: 'var(--viz-1)',
  project: 'var(--viz-2)',
  skills: 'var(--viz-3)',
  tools: 'var(--viz-4)',
  mcp: 'var(--viz-5)',
  messages: 'var(--viz-6)',
  toolResults: 'var(--viz-7)',
}

const LEGEND_GRID =
  'grid grid-cols-[8px_minmax(0,1fr)_auto_2.25rem] items-center gap-x-2 rounded-[4px] px-1 py-0.5 text-xs'

// 容量细条：读数在上面的文字里，这里只是示意，读屏跳过。
export function ContextCapacityBar({
  percent,
  compactAtPercent,
}: {
  percent: number
  compactAtPercent: number | null
}) {
  const { t } = useI18n()
  return (
    <div aria-hidden="true" className="relative h-1 rounded-full bg-surface-active">
      {percent > 0 && (
        <span
          className="absolute inset-y-0 left-0 min-w-1 rounded-full bg-content-secondary"
          style={{ width: `${Math.min(100, percent)}%` }}
        />
      )}
      {compactAtPercent !== null && (
        <span
          data-context-compact-line
          title={t('board:composer.context.compactAt', { percent: compactAtPercent })}
          className="absolute -top-[3px] -bottom-[3px] w-[2px] -translate-x-1/2 rounded-full bg-content"
          style={{ left: `${compactAtPercent}%` }}
        />
      )}
    </div>
  )
}

export function ContextBreakdownMeter({ breakdown }: { breakdown: ContextBreakdown }) {
  const { t } = useI18n()
  // 悬停彩条或图例时互相对应：其他分段变淡，对应的图例行高亮。
  const [active, setActive] = useState<ContextCategoryId | null>(null)
  const { contextWindow, usedTokens, freeTokens, compactAtPercent } = breakdown
  const parts = breakdown.categories.filter((category) => category.tokens > 0)
  // MCP 工具按需调用：连上了但没用到时占用是 0，这一行仍然留着，说清楚是“没占”而不是没统计到。
  const rows = breakdown.categories.filter(
    (category) => category.tokens > 0 || (category.id === 'mcp' && category.deferred > 0),
  )
  const labels: Record<ContextCategoryId, string> = {
    system: t('board:composer.context.system'),
    project: t('board:composer.context.project'),
    skills: t('board:composer.context.skills'),
    tools: t('board:composer.context.tools'),
    mcp: t('board:composer.context.mcp'),
    messages: t('board:composer.context.messages'),
    toolResults: t('board:composer.context.toolResults'),
  }
  const share = (value: number) => contextShareLabel(value, usedTokens)
  const hover = (id: ContextCategoryId) => ({
    onMouseEnter: () => setActive(id),
    onMouseLeave: () => setActive(null),
  })
  const capacity = [
    t('board:composer.context.free', { tokens: formatTokenCount(freeTokens) }),
    compactAtPercent !== null
      ? t('board:composer.context.compactAt', { percent: compactAtPercent })
      : '',
  ]
    .filter(Boolean)
    .join(' · ')
  return (
    <div className="grid gap-2" data-context-breakdown>
      <div className="grid gap-1.5">
        <ContextCapacityBar
          percent={contextWindow ? (usedTokens / contextWindow) * 100 : 0}
          compactAtPercent={compactAtPercent}
        />
        <span className="text-[11px] leading-4 text-content-muted" data-context-capacity>
          {capacity}
        </span>
      </div>
      {parts.length > 0 && (
        <div className="grid gap-1.5">
          <span className="text-[11px] leading-4 text-content-muted">
            {t('board:composer.context.composition')}
          </span>
          <div
            role="img"
            aria-label={parts
              .map((category) => `${labels[category.id]} ${share(category.tokens)}`)
              .join(', ')}
            className="flex h-2.5 w-full gap-[2px]"
          >
            {parts.map((category) => (
              <span
                key={category.id}
                data-context-segment={category.id}
                title={`${labels[category.id]} · ${formatTokenCount(category.tokens)} · ${share(category.tokens)}`}
                className={cn(
                  'min-w-[3px] rounded-[2px] transition-opacity duration-(--d-fast)',
                  active && active !== category.id && 'opacity-30',
                )}
                style={{
                  flexGrow: category.tokens,
                  flexBasis: 0,
                  backgroundColor: CATEGORY_COLORS[category.id],
                }}
                {...hover(category.id)}
              />
            ))}
          </div>
        </div>
      )}
      <ul className="m-0 grid list-none gap-0 p-0">
        {rows.map((category) => (
          <li key={category.id} className="contents">
            <div
              data-context-row={category.id}
              className={cn(LEGEND_GRID, active === category.id && 'bg-surface-hover')}
              {...hover(category.id)}
            >
              <span
                aria-hidden="true"
                className="size-2 rounded-[2px]"
                style={{ backgroundColor: CATEGORY_COLORS[category.id] }}
              />
              <span className="min-w-0 truncate text-content-secondary">
                {labels[category.id]}
                {category.id === 'tools' && category.count ? (
                  <span className="text-content-muted"> · {category.count}</span>
                ) : null}
              </span>
              <span className="font-medium text-content tabular-nums">
                {formatTokenCount(category.tokens)}
              </span>
              <span className="text-right text-content-muted tabular-nums">
                {share(category.tokens)}
              </span>
            </div>
            {category.servers.map((server) => {
              const serverLabel = [
                server.name,
                server.tools > 0
                  ? t('board:composer.context.toolCount', { count: server.tools })
                  : '',
                server.calls > 0
                  ? t('board:composer.context.callCount', { count: server.calls })
                  : '',
              ]
                .filter(Boolean)
                .join(' · ')
              return (
                <div
                  key={server.name}
                  data-context-server={server.name}
                  className={cn(LEGEND_GRID, 'py-0 text-content-muted')}
                >
                  <span aria-hidden="true" />
                  {/* 服务名可能很长：行内截断，完整内容放在悬停提示里。 */}
                  <span className="min-w-0 truncate" title={serverLabel}>
                    {serverLabel}
                  </span>
                  <span className="tabular-nums">{formatTokenCount(server.tokens)}</span>
                  <span />
                </div>
              )
            })}
            {category.deferred > 0 && (
              <div data-context-deferred className={cn(LEGEND_GRID, 'py-0 text-content-muted')}>
                <span aria-hidden="true" />
                <span className="col-span-3 min-w-0">
                  {t('board:composer.context.deferred', { count: category.deferred })}
                </span>
              </div>
            )}
          </li>
        ))}
      </ul>
      <p className="m-0 text-[11px] leading-4 text-content-muted" data-context-hint>
        {!breakdown.prefixKnown
          ? t('board:composer.context.prefixUnknown')
          : breakdown.totalKnown
            ? t('board:composer.context.estimated')
            : t('board:composer.context.allEstimated')}
      </p>
    </div>
  )
}
