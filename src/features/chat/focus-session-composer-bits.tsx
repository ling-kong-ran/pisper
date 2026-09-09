// 聚焦会话输入区的小型展示组件：排队托盘、资源调用芯片、状态指示灯、
// 手动压缩按钮与发送/停止按钮。从 FocusSession.tsx 拆出，样式逐字保留。
import { lazy, Suspense } from 'react'
import { Braces, Minimize2, RefreshCw, Send, Square, Undo2, Wrench, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useShortcutLabel } from '@/lib/shortcuts'
import { QueueSection } from '@/components/ai-elements/queue'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { EntityRecord, ResourceInvocation } from '@/types/chat'

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
        className="flex min-w-0 flex-col gap-1 text-xs text-[var(--text-secondary)]"
        data-pisper-queue-size={queuedInputs.length}
      >
        <div className="flex min-w-0 items-center gap-2 text-[11px] font-semibold text-[var(--star-strong)]">
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
                className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-2 border-b border-[var(--stroke-soft)] py-0.5 last:border-0"
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
                      className="rounded-md text-[var(--text-muted)] hover:text-[var(--text)]"
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
      className={`composer-resource-chip [&.workflow]:border-[var(--success)] [&.workflow]:bg-[var(--success-soft)] [&_button]:grid [&_button]:w-[20px] [&_button]:h-[20px] [&_button]:place-items-center [&_button]:border-0 [&_button]:rounded-[var(--r-xs)] [&_button]:bg-transparent [&_button]:text-[var(--text-muted)] [&_button]:cursor-pointer [&_button:hover]:bg-[var(--surface-hover)] [&_button:hover]:text-[var(--text)] inline-flex min-h-[28px] self-start items-center gap-[6px] [border:1px_solid_var(--blue)] rounded-[var(--r-sm)] bg-[var(--blue-soft)] text-[var(--text)] [padding:4px_6px_4px_8px] text-[12px] font-[600] ${invocation.kind}`}
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

export function ComposerStatusPill({
  compaction,
  streaming,
  statusLabel,
}: {
  compaction?: EntityRecord | null
  streaming?: boolean
  statusLabel?: string
}) {
  const { t } = useI18n()
  return (
    <div
      className={`focus-composer-status [.focus-session.has-conversation_&.idle]:hidden [&_>_i]:w-[7px] [&_>_i]:h-[7px] [&_>_i]:flex-none [&_>_i]:rounded-[50%] [&_>_i]:bg-[var(--text-muted)] [&.running]:text-[var(--success-strong)] [&.running_>_i]:bg-[var(--success)] [&.running_>_i]:shadow-[0_0_0_3px_var(--success-soft)] [&.running_>_i]:[animation:star-twinkle_1.1s_ease-in-out_infinite] inline-flex min-h-[22px] self-start items-center gap-[7px] [margin:0_0_-2px_5px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-pill)] bg-[var(--solid)] [padding:3px_9px_3px_7px] text-[var(--text-muted)] text-[11px] font-[600] shadow-[0_8px_18px_-16px_var(--shadow-strong)] ${compaction?.active ? 'compacting [.focus-composer-status&]:text-[var(--warning-strong)] [.focus-composer-status&_>_i]:bg-[var(--warning-strong)] [.focus-composer-status&_>_i]:shadow-[0_0_0_3px_var(--warning-soft)] [.focus-composer-status&_>_i]:[animation:star-twinkle_1.1s_ease-in-out_infinite]' : streaming ? 'running' : 'idle'}`}
      role="status"
      aria-live="polite"
    >
      <i aria-hidden="true" />
      <span className="min-w-0 max-w-[min(420px,60vw)] overflow-hidden text-ellipsis whitespace-nowrap">
        {compaction?.active ? (
          t('chat:focusSession.compactingContext')
        ) : streaming ? (
          <Suspense fallback={statusLabel || t('chat:focusSession.running')}>
            <ShinyText key={statusLabel || 'running'}>
              {statusLabel || t('chat:focusSession.running')}
            </ShinyText>
          </Suspense>
        ) : (
          t('chat:focusSession.waitingForInput')
        )}
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
      className="compact-context-trigger [&:hover:not(:disabled)]:border-[var(--accent-border)] [&:hover:not(:disabled)]:bg-[var(--accent-soft)] [&:hover:not(:disabled)]:text-[var(--star-strong)] disabled:[cursor:not-allowed] disabled:opacity-[.5] [.composer-tool-tray_&]:w-[38px] [.composer-tool-tray_&]:min-w-[38px] [.composer-tool-tray_&]:h-[38px] [.composer-tool-tray_&]:flex-none @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:min-w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[700px]:[.composer-tool-tray_&]:p-0 @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:min-w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px] grid w-[38px] h-[38px] flex-none place-items-center [border:1px_solid_transparent] rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-[var(--text-tertiary)] cursor-pointer"
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
  disabled,
  onAbort,
}: {
  streaming?: boolean
  queueing: boolean
  disabled: boolean
  onAbort: () => void
}) {
  const { t } = useI18n()
  const sendShortcut = useShortcutLabel('sendMessage')
  const sendLabel = t('chat:focusSession.sendMessage')
  return (
    <button
      type={streaming ? 'button' : 'submit'}
      className={`send-button grid !size-11 flex-none place-items-center rounded-[var(--r-sm)] border-0 bg-[var(--star)] text-[var(--on-accent)] transition-[var(--d1)] cursor-pointer hover:not(:disabled):bg-[var(--star-hover)] hover:not(:disabled):shadow-[var(--sh-star)] active:not(:disabled):scale-[.96] disabled:cursor-not-allowed disabled:border disabled:border-[var(--stroke)] disabled:bg-[var(--surface-muted)] disabled:text-[var(--text-muted)] ${streaming ? 'stop !bg-[var(--danger)] hover:not(:disabled):shadow-[0_0_0_3px_var(--danger-soft)]' : ''}`}
      title={
        streaming
          ? t('chat:focusSession.stop')
          : sendShortcut
            ? `${sendLabel} (${sendShortcut})`
            : sendLabel
      }
      aria-label={streaming ? t('chat:focusSession.stop') : t('chat:focusSession.sendMessage')}
      onClick={streaming ? onAbort : undefined}
      disabled={disabled}
    >
      {streaming ? (
        <Square size={16} fill="currentColor" />
      ) : queueing ? (
        <RefreshCw className="animate-spin" size={17} />
      ) : (
        <Send size={18} />
      )}
    </button>
  )
}
