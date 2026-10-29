// 标题栏舰队计量：◐ 运行 N ·（有排队能力时）◌ 排队 N · ▲ N 需要你 / ✓ 全部顺畅，所有页面常驻。
// 点击“需要你”标签展开 320px 浮层：等待最久事项的完整摘要 + 前往，其余事项列表与系统信息行。
// 浮层只负责跳转，没有任何“允许”入口；批准必须回到卡片或会话里完成。
import { useEffect, useRef, useState } from 'react'
import { ArrowRight, Check, TriangleAlert } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import type { StatusGlyphKind } from '@/components/app/status-glyph-kinds'
import { Kbd } from '@/components/ui/kbd'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import {
  useFleetSummary,
  useFleetSystemInfo,
  type FleetAttentionItem,
  type FleetSummaryResult,
} from '@/features/board/public'
import { formatCompactDuration, formatFileSize, formatTokenCount } from '@/lib/format'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'

const ATTENTION_GLYPHS: Partial<Record<FleetAttentionItem['status'], StatusGlyphKind>> = {
  approval: 'approval',
  question: 'question',
  failed: 'failed',
}

type FleetMeterProps = {
  onOpenSession: (sessionId: string) => void
  // 等待最久事项变化时通知外壳，⌥A 由外壳的快捷键层统一处理。
  onLongestWaitingChange?: (sessionId: string) => void
}

export function FleetMeter({ onOpenSession, onLongestWaitingChange }: FleetMeterProps) {
  const { t } = useI18n()
  const fleet = useFleetSummary()
  const [open, setOpen] = useState(false)
  const pulse = useAttentionPulse(fleet)
  const longestId = fleet.longestWaiting?.sessionId || ''
  useEffect(() => onLongestWaitingChange?.(longestId), [longestId, onLongestWaitingChange])
  const queueVisible = fleet.capabilities.sessionQueue
  const attention = fleet.attention
  const label = queueVisible
    ? t('navigation:shell.fleetWithQueue', {
        running: fleet.running,
        queued: fleet.queued,
        attention,
      })
    : t('navigation:shell.fleet', { running: fleet.running, attention })

  return (
    <div
      role="group"
      aria-label={label}
      className="flex h-8 flex-none items-center gap-2.5 font-mono text-xs font-medium whitespace-nowrap text-content-secondary tabular-nums"
    >
      <span className="flex items-center gap-1" title={t('navigation:shell.running')}>
        <StatusGlyph status="run" spin={fleet.running > 0} className="size-3.5 text-st-run" />
        <b className="font-semibold text-content">{fleet.running}</b>
        <span className="font-sans max-[1279px]:sr-only">{t('navigation:shell.running')}</span>
      </span>
      {queueVisible && (
        <span className="flex items-center gap-1" title={t('navigation:shell.queued')}>
          <StatusGlyph status="queued" className="size-3.5 text-st-queue" />
          <b className="font-semibold text-content">{fleet.queued}</b>
          <span className="font-sans max-[1279px]:sr-only">{t('navigation:shell.queued')}</span>
        </span>
      )}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            aria-label={
              attention > 0
                ? t('navigation:shell.attentionDetails', { count: attention })
                : t('navigation:shell.allClearDetails')
            }
            className="rounded-xs focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus pointer-coarse:min-h-11"
          >
            <span
              key={pulse}
              data-attention={attention > 0 || undefined}
              className={cn(
                'flex h-6 items-center gap-1 rounded-xs px-2 font-sans text-xs font-semibold',
                attention > 0
                  ? 'bg-st-attn-solid text-on-attn ring-1 ring-st-attn-ring ring-inset'
                  : 'bg-surface-muted text-content-secondary',
                // 新事项进入“需要你”时只脉冲一次；减少动效偏好由全局动效闸门压到瞬时。
                pulse > 0 && 'animate-[attn-pulse_900ms_var(--ease-enter)_1]',
              )}
            >
              {attention > 0 ? (
                <>
                  <TriangleAlert className="size-3.5" strokeWidth={2.25} aria-hidden="true" />
                  <span className="font-mono tabular-nums">{attention}</span>
                  {t('navigation:shell.attention')}
                </>
              ) : (
                <>
                  <Check className="size-3.5" strokeWidth={2.25} aria-hidden="true" />
                  {t('navigation:shell.allClear')}
                </>
              )}
            </span>
          </button>
        </PopoverTrigger>
        <PopoverContent align="end" className="w-80 gap-0 p-0">
          <FleetPanel
            fleet={fleet}
            open={open}
            onOpenSession={(sessionId) => {
              setOpen(false)
              onOpenSession(sessionId)
            }}
          />
        </PopoverContent>
      </Popover>
    </div>
  )
}

// 新的需要你事项出现时累加一次脉冲序号；首次载入的既有事项不算“新进入”。
function useAttentionPulse(fleet: FleetSummaryResult) {
  const seenRef = useRef<Set<string> | null>(null)
  const [pulse, setPulse] = useState(0)
  const ids = fleet.attentionItems.map((item) => item.sessionId).join('\n')
  const loading = fleet.isLoading
  useEffect(() => {
    if (loading) return
    const next = new Set(ids ? ids.split('\n') : [])
    const previous = seenRef.current
    seenRef.current = next
    if (previous && [...next].some((id) => !previous.has(id))) setPulse((value) => value + 1)
  }, [ids, loading])
  return pulse
}

function useNow(enabled: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!enabled) return
    setNow(Date.now())
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [enabled])
  return now
}

function FleetPanel({
  fleet,
  open,
  onOpenSession,
}: {
  fleet: FleetSummaryResult
  open: boolean
  onOpenSession: (sessionId: string) => void
}) {
  const { t } = useI18n()
  const jumpShortcut = useShortcutLabel('jumpToAttention')
  const now = useNow(open)
  const system = useFleetSystemInfo(open)
  const statusLabels: Record<string, string> = {
    approval: t('navigation:shell.statusApproval'),
    question: t('navigation:shell.statusQuestion'),
    failed: t('navigation:shell.statusFailed'),
  }
  const first = fleet.longestWaiting
  const rest = fleet.attentionItems.filter((item) => item !== first)
  const waited = (item: FleetAttentionItem) =>
    item.since === null
      ? ''
      : t('navigation:shell.waited', { duration: formatCompactDuration(now - item.since) })
  const systemParts = [
    system.data?.todayTokens != null
      ? t('navigation:shell.todayTokens', { tokens: formatTokenCount(system.data.todayTokens) })
      : '',
    system.data?.memoryBytes != null
      ? t('navigation:shell.memory', { size: formatFileSize(system.data.memoryBytes) })
      : '',
  ].filter(Boolean)

  return (
    <div className="flex flex-col" aria-label={t('navigation:shell.fleetPanel')} role="group">
      {fleet.isLoading ? (
        <p role="status" className="px-3 py-4 text-sm text-content-muted">
          {t('navigation:shell.attentionLoading')}
        </p>
      ) : !first ? (
        <div className="flex flex-col gap-1 px-3 py-4">
          <p className="flex items-center gap-1.5 text-sm font-medium text-content">
            <Check className="size-4 text-st-done" aria-hidden="true" />
            {t('navigation:shell.noAttention')}
          </p>
          <p className="text-xs text-content-muted">
            {fleet.error
              ? t('navigation:shell.attentionError')
              : t('navigation:shell.noAttentionHint')}
          </p>
        </div>
      ) : (
        <>
          <section className="flex flex-col gap-2 border-b border-stroke-soft p-3">
            <p className="text-xs font-medium text-content-muted">
              {t('navigation:shell.longestWaiting')}
            </p>
            <div className="flex items-center gap-1.5 text-xs text-st-attn">
              <StatusGlyph
                status={ATTENTION_GLYPHS[first.status] || 'approval'}
                className={cn('size-3.5', first.status === 'failed' && 'text-st-fail')}
              />
              <span className={cn('font-medium', first.status === 'failed' && 'text-st-fail')}>
                {statusLabels[first.status] || first.status}
              </span>
              {first.severity === 'critical' && (
                <span className="rounded-xs border border-st-fail-border bg-st-fail-soft px-1 text-st-fail">
                  {t('navigation:shell.critical')}
                </span>
              )}
              <span className="ml-auto font-mono text-content-muted tabular-nums">
                {waited(first)}
              </span>
            </div>
            <p className="text-sm font-semibold break-words text-content">
              {first.title || t('navigation:shell.untitledSession')}
            </p>
            {first.summary && (
              <pre className="max-h-24 overflow-y-auto rounded-sm border border-well-stroke bg-well px-2 py-1.5 font-mono text-xs whitespace-pre-wrap break-all text-well-text">
                {first.summary}
              </pre>
            )}
            <button
              type="button"
              className="flex h-8 items-center justify-center gap-1.5 self-end rounded-sm border border-stroke-strong bg-key px-2.5 text-xs font-medium text-content shadow-key hover:bg-surface-hover focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus pointer-coarse:min-h-11"
              onClick={() => onOpenSession(first.sessionId)}
            >
              {t('navigation:shell.goToAttention')}
              {jumpShortcut && <Kbd aria-hidden="true">{jumpShortcut}</Kbd>}
              <ArrowRight className="size-3.5" aria-hidden="true" />
            </button>
          </section>
          {rest.length > 0 && (
            <section className="flex max-h-60 flex-col overflow-y-auto py-1">
              <p className="px-3 pt-1.5 pb-1 text-xs font-medium text-content-muted">
                {t('navigation:shell.otherAttention')}
              </p>
              <ul className="flex flex-col">
                {rest.map((item) => (
                  <li key={item.sessionId}>
                    <button
                      type="button"
                      className="flex w-full min-w-0 items-center gap-2 px-3 py-1.5 text-left text-sm hover:bg-surface-hover focus-visible:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:min-h-11"
                      onClick={() => onOpenSession(item.sessionId)}
                    >
                      <StatusGlyph
                        status={ATTENTION_GLYPHS[item.status] || 'approval'}
                        label={statusLabels[item.status]}
                        className={cn(
                          'size-3.5 flex-none',
                          item.status === 'failed' ? 'text-st-fail' : 'text-st-attn',
                        )}
                      />
                      <span className="min-w-0 flex-1 truncate text-content">
                        {item.title || t('navigation:shell.untitledSession')}
                      </span>
                      <span className="flex-none font-mono text-xs text-content-muted tabular-nums">
                        {waited(item)}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          )}
        </>
      )}
      {systemParts.length > 0 && (
        <p className="border-t border-stroke-soft px-3 py-2 font-mono text-xs text-content-muted tabular-nums">
          <span className="sr-only">{t('navigation:shell.systemInfo')}: </span>
          {systemParts.join(' · ')}
        </p>
      )}
    </div>
  )
}
