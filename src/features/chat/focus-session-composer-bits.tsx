// 聚焦会话输入区的小型展示组件：排队托盘、资源调用芯片、状态指示灯、
// 手动压缩按钮与发送/停止按钮。从 FocusSession.tsx 拆出，样式逐字保留。
import { Braces, Minimize2, RefreshCw, Send, Square, Wrench, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { QueueSection } from '@/components/ai-elements/queue'
import type { EntityRecord, ResourceInvocation } from '@/types/chat'

export function QueuedInputsTray({ queuedInputs }: { queuedInputs: EntityRecord[] }) {
  const { t } = useI18n()
  return (
    <QueueSection asChild defaultOpen>
      <div
        className="queued-input-tray [&_>_span]:flex-none [&_>_span]:text-[var(--star-strong)] [&_>_span]:font-[600] [&_>_small]:max-w-[240px] [&_>_small]:overflow-hidden [&_>_small]:rounded-[var(--r-pill)] [&_>_small]:bg-[var(--surface-muted)] [&_>_small]:p-[4px_8px] [&_>_small]:text-[var(--text-secondary)] [&_>_small]:text-ellipsis [&_>_small]:whitespace-nowrap [&_>_em]:flex-none [&_>_em]:[font-style:normal] flex min-w-0 items-center gap-[6px] overflow-hidden text-[var(--text-muted)] text-[11px]"
        data-pisper-queue-size={queuedInputs.length}
      >
        <span>{t('chat:focusSession.sentToTheRunningAgent')}</span>
        {queuedInputs.slice(-3).map((item, index) => (
          <small key={item.id || `${item.behavior}-${index}`} title={item.text}>
            {item.text}
          </small>
        ))}
        {queuedInputs.length > 3 && (
          <em>{t('chat:focusSession.countMore', { count: queuedInputs.length - 3 })}</em>
        )}
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
}: {
  compaction?: EntityRecord | null
  streaming?: boolean
}) {
  const { t } = useI18n()
  return (
    <div
      className={`focus-composer-status [.focus-session.has-conversation_&.idle]:hidden [&_>_i]:w-[7px] [&_>_i]:h-[7px] [&_>_i]:flex-none [&_>_i]:rounded-[50%] [&_>_i]:bg-[var(--text-muted)] [&.running]:text-[var(--success-strong)] [&.running_>_i]:bg-[var(--success)] [&.running_>_i]:shadow-[0_0_0_3px_var(--success-soft)] [&.running_>_i]:[animation:star-twinkle_1.1s_ease-in-out_infinite] inline-flex min-h-[22px] self-start items-center gap-[7px] [margin:0_0_-2px_5px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-pill)] bg-[var(--solid)] [padding:3px_9px_3px_7px] text-[var(--text-muted)] text-[11px] font-[600] shadow-[0_8px_18px_-16px_var(--shadow-strong)] ${compaction?.active ? 'compacting [.focus-composer-status&]:text-[var(--warning-strong)] [.focus-composer-status&_>_i]:bg-[var(--warning-strong)] [.focus-composer-status&_>_i]:shadow-[0_0_0_3px_var(--warning-soft)] [.focus-composer-status&_>_i]:[animation:star-twinkle_1.1s_ease-in-out_infinite]' : streaming ? 'running' : 'idle'}`}
      role="status"
      aria-live="polite"
    >
      <i aria-hidden="true" />
      <span>
        {compaction?.active
          ? t('chat:focusSession.compactingContext')
          : streaming
            ? t('chat:focusSession.running')
            : t('chat:focusSession.waitingForInput')}
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
  return (
    <button
      type={streaming ? 'button' : 'submit'}
      className={`send-button grid !size-11 flex-none place-items-center rounded-[var(--r-sm)] border-0 bg-[var(--star)] text-[var(--on-accent)] transition-[var(--d1)] cursor-pointer hover:not(:disabled):bg-[var(--star-hover)] hover:not(:disabled):shadow-[var(--sh-star)] active:not(:disabled):scale-[.96] disabled:cursor-not-allowed disabled:border disabled:border-[var(--stroke)] disabled:bg-[var(--surface-muted)] disabled:text-[var(--text-muted)] ${streaming ? 'stop !bg-[var(--danger)] hover:not(:disabled):shadow-[0_0_0_3px_var(--danger-soft)]' : ''}`}
      title={streaming ? t('chat:focusSession.stop') : t('chat:focusSession.sendMessage')}
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
