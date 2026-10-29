// 聚焦会话输入区的小型展示组件：排队托盘、资源调用芯片、输入框顶部的运行条、
// 手动压缩按钮与按草稿、队列和准备状态选择的发送/停止按钮。
import { lazy, Suspense, useEffect, useState } from 'react'
import { Braces, Minimize2, RefreshCw, ArrowUp, Square, Undo2, Wrench, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useShortcutLabel } from '@/lib/shortcuts'
import { QueueSection } from '@/components/ai-elements/queue'
import { Button } from '@/components/ui/button'
import { Kbd } from '@/components/ui/kbd'
import { OrbitSpinner } from '@/components/ui/spinner'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { formatStepDuration } from '@/features/chat/run-activity-presentation'
import { runDurationMs } from '@/features/chat/run-activity'
import type { EntityRecord, ResourceInvocation } from '@/types/chat'

// 输入区工具图标键：桌面 30px，手机与粗指针 44px 触控目标；多处复用同一组工具类，避免重复生成样式。
export const COMPOSER_ICON_KEY_CLASSES =
  'grid size-[30px] min-w-[30px] flex-none cursor-pointer place-items-center rounded-sm border-0 bg-transparent text-content-muted transition-colors hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-50 max-[650px]:size-11 max-[650px]:min-w-11 pointer-coarse:size-11 pointer-coarse:min-w-11'

// 状态文案切换时的一次性光泽扫过（ShinyText 动画只跑一遍），懒加载避免进入主包。
const ShinyText = lazy(() =>
  import('@/components/react-bits/ShinyText').then((module) => ({ default: module.ShinyText })),
)

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
      <span aria-hidden="true" className="text-content-faint">
        ·
      </span>
      <span aria-hidden="true" className="shrink-0 text-content-secondary tabular-nums">
        {formatStepDuration(elapsed, true)}
      </span>
    </>
  )
}

// 输入框顶部的集成运行条：轨道指示 + 强调色「运行中」+ 工具/计时/命令 + 真实停止按钮，
// 底边一条强调色进度细线。空闲时整条隐藏；压缩上下文时改用警示色且不提供停止。
export function ComposerStatusPill({
  compaction,
  streaming,
  statusLabel,
  startedAt,
  activity,
  stopping = false,
  onStop,
}: {
  compaction?: EntityRecord | null
  streaming?: boolean
  statusLabel?: string
  startedAt?: string | null
  activity?: EntityRecord | null
  stopping?: boolean
  onStop?: () => void
}) {
  const { t } = useI18n()
  const compacting = Boolean(compaction?.active)
  const tool = !compacting && activity?.type === 'tool' && activity.status === 'running'
  const target = tool ? runningToolTarget(activity) : ''
  return (
    <div
      className={`focus-composer-status [&.idle]:hidden [&.running]:text-[var(--success-strong)] [&.running]:border-star-border [&.running]:bg-star-soft relative flex h-[34px] min-w-0 shrink-0 items-center gap-2 rounded-t-[inherit] border-b pr-1 pl-4 text-xs text-content-muted pointer-coarse:h-11 max-[650px]:h-11 max-[650px]:pr-0 max-[650px]:pl-3.5 max-[650px]:text-[13px] ${compaction?.active ? 'compacting border-stroke-soft bg-warning-soft text-[var(--warning-strong)]' : streaming ? 'running' : 'idle'}`}
    >
      <span role="status" aria-live="polite" className="flex min-w-0 flex-1 items-center gap-2">
        <OrbitSpinner className={`size-3.5 ${compacting ? 'text-warning-strong' : ''}`} />
        <span
          className={`shrink-0 font-medium ${compacting ? 'text-warning-strong' : 'text-accent-fg'}`}
        >
          {compacting ? t('chat:focusSession.compactingContext') : t('chat:focusSession.running')}
        </span>
        {streaming && <RunElapsed startedAt={startedAt} />}
        {tool ? (
          <>
            <span aria-hidden="true" className="text-content-faint">
              ·
            </span>
            <span className="shrink-0 font-mono font-medium text-content-secondary">
              {String(activity?.name || '')}
            </span>
            {target && (
              <span className="min-w-0 truncate font-mono text-content-muted" title={target}>
                {target}
              </span>
            )}
          </>
        ) : (
          streaming &&
          statusLabel && (
            <>
              <span aria-hidden="true" className="text-content-faint">
                ·
              </span>
              <span className="min-w-0 truncate text-content-secondary">
                <Suspense fallback={statusLabel}>
                  <ShinyText key={statusLabel}>{statusLabel}</ShinyText>
                </Suspense>
              </span>
            </>
          )
        )}
      </span>
      {streaming && !compacting && onStop && (
        <button
          type="button"
          className="inline-flex h-6 shrink-0 cursor-pointer items-center gap-1.5 rounded-sm px-1.5 text-xs text-content-secondary transition-colors hover:bg-surface-hover hover:text-content disabled:cursor-default disabled:opacity-60 max-[650px]:h-11 max-[650px]:rounded-none max-[650px]:rounded-tr-[inherit] max-[650px]:border-l max-[650px]:border-star-border max-[650px]:px-3.5 max-[650px]:text-[13px] max-[650px]:font-medium max-[650px]:text-danger max-[650px]:active:bg-danger-soft pointer-coarse:h-11"
          aria-label={t('chat:focusSession.stopRun')}
          title={t('chat:focusSession.stopRunShortcut')}
          aria-busy={stopping || undefined}
          disabled={stopping}
          onClick={onStop}
        >
          <Kbd className="max-[650px]:hidden pointer-coarse:hidden">Esc</Kbd>
          <Square size={10} fill="currentColor" className="hidden max-[650px]:block" />
          {stopping ? t('chat:focusSession.stopping') : t('chat:focusSession.stop')}
        </button>
      )}
      <span
        aria-hidden="true"
        className="pointer-events-none absolute inset-x-0 -bottom-px h-px overflow-hidden"
      >
        <span
          className={`block h-full w-[38%] animate-[hairline-sweep_1.4s_var(--ease)_infinite] ${compacting ? 'bg-warning-strong' : 'bg-star'}`}
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
  onAbort,
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
  onAbort: () => void
}) {
  const { t } = useI18n()
  const sendShortcut = useShortcutLabel('sendMessage')
  const stopAction = Boolean(streaming && !(hasDraft && canQueue) && !preparing)
  const busy = queueing || submitting || stopping || preparing
  const unavailable = disabled || busy || Boolean(blockedReason)
  const label = preparing
    ? t('chat:focusSession.preparingAttachments')
    : stopping
      ? t('chat:focusSession.stopping')
      : stopAction
        ? t('chat:focusSession.stop')
        : t('chat:focusSession.sendMessage')
  return (
    <button
      type={stopAction ? 'button' : 'submit'}
      className="send-button grid size-8 min-w-8 flex-none cursor-pointer place-items-center rounded-md border border-transparent bg-star text-on-accent shadow-primary transition-colors hover:enabled:bg-star-hover active:enabled:translate-y-px disabled:cursor-not-allowed disabled:opacity-40 disabled:shadow-none aria-disabled:opacity-50 data-[action=stop]:border-control data-[action=stop]:bg-key data-[action=stop]:text-content data-[action=stop]:shadow-key data-[action=stop]:hover:enabled:bg-surface-hover max-[650px]:size-11 max-[650px]:min-w-11 max-[650px]:rounded-xl pointer-coarse:size-11 pointer-coarse:min-w-11"
      data-action={stopAction ? 'stop' : 'send'}
      title={
        blockedReason ||
        (!stopAction && !busy && sendShortcut ? `${label} (${sendShortcut})` : label)
      }
      aria-label={blockedReason ? `${label}: ${blockedReason}` : label}
      aria-busy={busy || undefined}
      aria-disabled={unavailable || undefined}
      onClick={(event) => {
        if (unavailable) {
          event.preventDefault()
          return
        }
        if (stopAction) onAbort()
      }}
      disabled={!blockedReason && unavailable}
    >
      {busy ? (
        <RefreshCw className="animate-spin" size={17} />
      ) : stopAction ? (
        <Square size={11} fill="currentColor" />
      ) : (
        <ArrowUp size={17} strokeWidth={2.1} />
      )}
    </button>
  )
}
