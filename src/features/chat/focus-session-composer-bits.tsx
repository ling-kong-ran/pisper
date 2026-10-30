// 聚焦会话输入区的小型展示组件：排队托盘、资源调用芯片、输入框顶部的运行条、
// 手动压缩按钮与按草稿、队列和准备状态选择的发送/停止按钮。
import { useEffect, useState } from 'react'
import { Braces, Minimize2, RefreshCw, ArrowUp, Square, Undo2, Wrench, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { formatCompactDuration, formatTokenCount } from '@/lib/format'
import { useShortcutLabel } from '@/lib/shortcuts'
import { QueueSection } from '@/components/ai-elements/queue'
import { Button } from '@/components/ui/button'
import { Kbd } from '@/components/ui/kbd'
import { OrbitSpinner } from '@/components/ui/spinner'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { runDurationMs } from '@/features/chat/run-activity'
import type { EntityRecord, ResourceInvocation } from '@/types/chat'

// 输入区工具图标键：桌面 30px，手机与粗指针 44px 触控目标；多处复用同一组工具类，避免重复生成样式。
export const COMPOSER_ICON_KEY_CLASSES =
  'grid size-[30px] min-w-[30px] flex-none cursor-pointer place-items-center rounded-sm border-0 bg-transparent text-content-muted transition-colors hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-50 max-[650px]:size-11 max-[650px]:min-w-11 pointer-coarse:size-11 pointer-coarse:min-w-11'

export function QueuedInputsTray({
  queuedInputs,
  withdrawingInputIds = [],
  onWithdraw,
}: {
  queuedInputs: EntityRecord[]
  withdrawingInputIds?: string[]
  onWithdraw?: (inputId: string) => Promise<void> | void
}) {
  const { t } = useI18n()
  const label = t('chat:focusSession.sentToTheRunningAgent')
  const withdrawLabel = t('chat:focusSession.withdrawQueuedInput')
  return (
    <QueueSection asChild defaultOpen>
      <div
        className="flex min-w-0 flex-col gap-1 rounded-md border border-stroke bg-surface-subtle py-1.5 pr-1 pl-3 text-xs text-content-secondary"
        data-pisper-queue-size={queuedInputs.length}
      >
        <div className="flex min-w-0 items-center gap-2 font-medium text-accent-fg">
          <span className="min-w-0 [overflow-wrap:anywhere]">{label}</span>
          <span className="shrink-0 tabular-nums">{queuedInputs.length}</span>
        </div>
        <ul
          className="m-0 max-h-36 min-w-0 list-none overflow-x-hidden overflow-y-auto overscroll-contain p-0"
          aria-label={label}
          tabIndex={queuedInputs.length > 3 ? 0 : undefined}
        >
          {queuedInputs.map((item, index) => {
            const inputId = typeof item.id === 'string' ? item.id : ''
            const pending = withdrawingInputIds.includes(inputId)
            return (
              <li
                key={inputId || `${item.behavior}-${index}`}
                className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-2 border-b border-stroke-soft py-0.5 last:border-0"
              >
                <span
                  className="line-clamp-2 min-w-0 whitespace-pre-wrap [overflow-wrap:anywhere]"
                  title={item.text}
                >
                  {/* 运行中 ↵ 排队（followUp，本轮结束后再交给代理）与 ⌥↵ 插话（steer，下一步就读到）分开标注。 */}
                  {(item.behavior === 'followUp' || item.behavior === 'steer') && (
                    <span
                      className="mr-1.5 inline-flex h-4.5 items-center rounded-xs bg-surface-muted px-1 align-[1px] text-[11px] font-medium text-content-secondary"
                      data-queued-behavior={item.behavior}
                    >
                      {item.behavior === 'followUp'
                        ? t('board:composer.queuedFollowUp')
                        : t('board:composer.queuedSteer')}
                    </span>
                  )}
                  {item.text}
                </span>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-lg"
                      className="text-content-muted hover:text-content max-[650px]:size-11"
                      aria-label={withdrawLabel}
                      aria-busy={pending || undefined}
                      disabled={!inputId || !onWithdraw || pending}
                      onClick={() => void onWithdraw?.(inputId)}
                    >
                      <Undo2 size={16} aria-hidden="true" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>{withdrawLabel}</TooltipContent>
                </Tooltip>
              </li>
            )
          })}
        </ul>
      </div>
    </QueueSection>
  )
}

export function ComposerResourceChip({
  invocation,
  onRemove,
}: {
  invocation: ResourceInvocation
  onRemove: () => void
}) {
  const { t } = useI18n()
  return (
    <div
      className={`composer-resource-chip [&.workflow]:border-success-border [&.workflow]:bg-success-soft [&.workflow]:text-success-strong [&_button]:grid [&_button]:size-6 [&_button]:place-items-center [&_button]:rounded-xs [&_button]:text-content-muted [&_button]:hover:bg-surface-hover [&_button]:hover:text-content inline-flex h-7 self-start items-center gap-1.5 rounded-sm border border-star-border bg-star-soft pr-0.5 pl-2 text-xs font-medium text-star-strong max-[650px]:h-11 max-[650px]:[&_button]:size-11 ${invocation.kind}`}
    >
      {invocation.kind === 'tool' ? <Wrench size={13} /> : <Braces size={13} />}
      <span>
        {invocation.kind === 'skill'
          ? 'Skill'
          : invocation.kind === 'tool'
            ? t('chat:resourcePicker.tool')
            : t('chat:resourcePicker.workflow')}{' '}
        · {invocation.resourceName}
      </span>
      <button type="button" aria-label={t('chat:resourcePicker.remove')} onClick={onRemove}>
        <X size={13} />
      </button>
    </div>
  )
}

// 运行条正在执行的工具目标：只取真实参数（命令或路径），取不到就不显示。
function runningToolTarget(activity?: EntityRecord | null) {
  const args = activity?.args && typeof activity.args === 'object' ? activity.args : {}
  const target = activity?.name === 'bash' ? args.command : args.path || args.pattern || args.url
  return typeof target === 'string' ? target.replace(/\s+/g, ' ').trim() : ''
}

// 每秒刷新只留在这段计时文字内，避免整个输入区随计时器重渲染；缺少开始时间时不以 0 冒充。
// 与会话页头、泳道条同一种紧凑写法（27s、1m07s），同一个运行不出现两种计时格式。
function RunElapsed({ startedAt }: { startedAt?: string | null }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [])
  const elapsed = runDurationMs(startedAt, null, now)
  if (!elapsed) return null
  return (
    <>
      <span aria-hidden="true" className="text-content-muted">
        ·
      </span>
      <span aria-hidden="true" className="shrink-0 font-mono font-medium text-content tabular-nums">
        {formatCompactDuration(elapsed)}
      </span>
    </>
  )
}

// 会话页运行条右侧的实时读数（规格 4.3）：上下文、Token、缓存命中、排队条数。
// 只显示真实拿到的数；拿不到的那一项整段不出现，不用 0 或“—”占位。
export type ComposerRunMetrics = {
  contextPercent: number | null
  tokens: number | null
  cacheHitRate: number | null
  queued: number
}

function RunMetrics({ metrics }: { metrics: ComposerRunMetrics }) {
  const { t } = useI18n()
  // 会话路由随页加载看板命名空间，运行条读数文案在那里（不占入口常驻的 chat 命名空间）。
  const parts = [
    metrics.contextPercent != null && {
      key: 'context',
      text: t('board:composer.context', { percent: Math.round(metrics.contextPercent) }),
      hide: '@max-[520px]/runbar:hidden',
    },
    metrics.tokens != null &&
      metrics.tokens > 0 && {
        key: 'tokens',
        text: t('board:composer.tokens', { tokens: formatTokenCount(metrics.tokens) }),
        hide: '@max-[600px]/runbar:hidden',
      },
    metrics.cacheHitRate != null && {
      key: 'cache',
      text: t('board:composer.cacheHit', { percent: Math.round(metrics.cacheHitRate) }),
      hide: '@max-[680px]/runbar:hidden',
    },
    metrics.queued > 0 && {
      key: 'queued',
      text: t('board:composer.queued', { count: metrics.queued }),
      hide: '',
    },
  ].filter((part): part is { key: string; text: string; hide: string } => Boolean(part))
  if (!parts.length) return null
  return (
    <span
      className="flex flex-none items-center gap-2 text-content-secondary tabular-nums"
      data-run-metrics
    >
      {parts.map((part) => (
        <span key={part.key} className={`flex flex-none items-center gap-2 ${part.hide}`}>
          <span aria-hidden="true" className="text-content-muted">
            ·
          </span>
          {part.text}
        </span>
      ))}
    </span>
  )
}

// 输入框顶部的集成运行条：运行中字形 + “运行中”（运行青）+ 工具/计时/命令 + 实时读数 + 真实停止按钮，
// 底边一条运行青的不确定进度线（只动 transform，减少动效时隐藏）。空闲时整条隐藏；压缩上下文时改用警示色且不提供停止。
// 运行条是唯一的停止入口：发送键不兼作停止。
export function ComposerStatusPill({
  compaction,
  streaming,
  statusLabel,
  startedAt,
  activity,
  stopping = false,
  onStop,
  metrics,
}: {
  compaction?: EntityRecord | null
  streaming?: boolean
  statusLabel?: string
  startedAt?: string | null
  activity?: EntityRecord | null
  stopping?: boolean
  onStop?: () => void
  metrics?: ComposerRunMetrics | null
}) {
  const { t } = useI18n()
  const compacting = Boolean(compaction?.active)
  const tool = !compacting && activity?.type === 'tool' && activity.status === 'running'
  const target = tool ? runningToolTarget(activity) : ''
  return (
    <div
      className={`focus-composer-status @container/runbar [&.idle]:hidden [&.running]:border-st-run-border [&.running]:bg-st-run-soft relative flex h-[34px] min-w-0 shrink-0 items-center gap-2 rounded-t-[inherit] border-b pr-1 pl-3.5 text-xs text-content-secondary pointer-coarse:h-11 max-[650px]:h-11 max-[650px]:pr-0 max-[650px]:pl-3.5 max-[650px]:text-[13px] ${compaction?.active ? 'compacting border-stroke-soft bg-warning-soft text-[var(--warning-strong)]' : streaming ? 'running' : 'idle'}`}
      data-run-bar={compacting ? 'compacting' : streaming ? 'running' : undefined}
    >
      <span role="status" aria-live="polite" className="flex min-w-0 flex-1 items-center gap-2">
        {compacting ? (
          <OrbitSpinner className="size-3.5 text-warning-strong" />
        ) : (
          <StatusGlyph status="run" className="size-3.5 text-st-run" />
        )}
        <span
          className={`shrink-0 font-semibold ${compacting ? 'text-warning-strong' : 'text-st-run'}`}
        >
          {compacting ? t('chat:focusSession.compactingContext') : t('chat:focusSession.running')}
        </span>
        {tool ? (
          <>
            <span aria-hidden="true" className="text-content-muted">
              ·
            </span>
            <span className="shrink-0 font-mono font-semibold text-content">
              {String(activity?.name || '')}
            </span>
          </>
        ) : null}
        {streaming && <RunElapsed startedAt={startedAt} />}
        {tool
          ? target && (
              <>
                <span aria-hidden="true" className="text-content-muted">
                  ·
                </span>
                <span className="min-w-0 truncate font-mono text-content-secondary" title={target}>
                  {target}
                </span>
              </>
            )
          : streaming &&
            statusLabel && (
              <>
                <span aria-hidden="true" className="text-content-muted">
                  ·
                </span>
                <span className="min-w-0 truncate text-content-secondary">{statusLabel}</span>
              </>
            )}
        {streaming && !compacting && metrics && <RunMetrics metrics={metrics} />}
      </span>
      {streaming && onStop && (
        <button
          type="button"
          className="inline-flex h-6 shrink-0 cursor-pointer items-center gap-1.5 rounded-sm bg-solid pr-1 pl-2 text-xs font-medium text-content shadow-[inset_0_0_0_1px_var(--stroke-strong)] transition-colors hover:bg-surface-hover disabled:cursor-default disabled:opacity-60 max-[650px]:h-11 max-[650px]:rounded-none max-[650px]:rounded-tr-[inherit] max-[650px]:bg-transparent max-[650px]:px-3.5 max-[650px]:text-[13px] max-[650px]:text-st-fail max-[650px]:shadow-[inset_1px_0_0_var(--st-run-border)] max-[650px]:active:bg-danger-soft pointer-coarse:h-11"
          aria-label={stopping ? t('chat:focusSession.stopping') : t('chat:focusSession.stop')}
          title={t('chat:focusSession.stopRunShortcut')}
          aria-busy={stopping || undefined}
          disabled={stopping}
          onClick={onStop}
          data-run-stop
        >
          <Square size={10} fill="currentColor" className="text-st-fail" aria-hidden="true" />
          {stopping ? t('chat:focusSession.stopping') : t('chat:focusSession.stop')}
          <Kbd className="max-[650px]:hidden pointer-coarse:hidden">Esc</Kbd>
        </button>
      )}
      <span
        aria-hidden="true"
        className="pointer-events-none absolute inset-x-0 -bottom-px h-[2px] overflow-hidden [:root[data-motion=reduced]_&]:hidden motion-reduce:[:root:not([data-motion=full])_&]:hidden"
      >
        <span
          className={`block h-full w-[30%] animate-[hairline-sweep_1.8s_var(--ease)_infinite] ${compacting ? 'bg-warning-strong' : 'bg-st-run-fill'}`}
        />
      </span>
    </div>
  )
}

export function CompactContextButton({
  streaming,
  compactingManually,
  compactionActive,
  disabled,
  onCompact,
}: {
  streaming?: boolean
  compactingManually: boolean
  compactionActive: boolean
  disabled: boolean
  onCompact: () => void
}) {
  const { t } = useI18n()
  return (
    <button
      type="button"
      className={`compact-context-trigger ${COMPOSER_ICON_KEY_CLASSES}`}
      title={
        streaming
          ? t('chat:focusSession.manualCompactionWaitForRun')
          : compactingManually || compactionActive
            ? t('chat:focusSession.compactingContext')
            : t('chat:focusSession.compactContextNow')
      }
      aria-label={t('chat:focusSession.compactContextNow')}
      disabled={disabled}
      onClick={onCompact}
    >
      {compactingManually || compactionActive ? (
        <RefreshCw className="animate-spin" size={14} />
      ) : (
        <Minimize2 size={14} />
      )}
    </button>
  )
}

export function ComposerSendButton({
  streaming,
  queueing,
  submitting = false,
  stopping = false,
  preparing = false,
  hasDraft = false,
  canQueue = false,
  blockedReason,
  disabled,
}: {
  streaming?: boolean
  queueing: boolean
  submitting?: boolean
  stopping?: boolean
  preparing?: boolean
  hasDraft?: boolean
  canQueue?: boolean
  blockedReason?: string
  disabled: boolean
}) {
  const { t } = useI18n()
  const sendShortcut = useShortcutLabel('sendMessage')
  // 发送键只负责发送；运行中有草稿时把补充说明排入队列，停止只由运行条上的停止键负责。
  const waitingForDraft = Boolean(streaming && !(hasDraft && canQueue))
  const busy = queueing || submitting || stopping || preparing
  const unavailable = disabled || busy || waitingForDraft || Boolean(blockedReason)
  const label = preparing
    ? t('chat:focusSession.preparingAttachments')
    : stopping
      ? t('chat:focusSession.stopping')
      : t('chat:focusSession.sendMessage')
  return (
    <button
      type="submit"
      className="send-button grid size-8 min-w-8 flex-none cursor-pointer place-items-center rounded-md border border-transparent bg-star text-on-accent shadow-primary transition-colors hover:enabled:bg-star-hover active:enabled:translate-y-px disabled:cursor-not-allowed disabled:opacity-40 disabled:shadow-none aria-disabled:opacity-50 max-[650px]:size-11 max-[650px]:min-w-11 max-[650px]:rounded-xl pointer-coarse:size-11 pointer-coarse:min-w-11"
      title={blockedReason || (!busy && sendShortcut ? `${label} (${sendShortcut})` : label)}
      aria-label={blockedReason ? `${label}: ${blockedReason}` : label}
      aria-busy={busy || undefined}
      aria-disabled={unavailable || undefined}
      onClick={(event) => {
        if (unavailable) event.preventDefault()
      }}
      disabled={!blockedReason && unavailable}
    >
      {busy ? (
        <RefreshCw className="animate-spin" size={17} />
      ) : (
        <ArrowUp size={17} strokeWidth={2.1} />
      )}
    </button>
  )
}
