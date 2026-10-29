// 高风险批准的确认层：critical（命令守卫拦截等）或详情未知的审批，在卡片上点“允许…”或按 A
// 都只会打开这里。确认层展示完整参数、工作目录与守卫原因，按住按钮 0.6 秒或在层内按 ↵ 才真正批准。
// 桌面是居中对话框，手机外壳是带拖拽条的底部表单。⌥A 与舰队浮层永远不会走到这里之外的批准路径。
import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react'
import { ShieldAlert } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Kbd } from '@/components/ui/kbd'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import type { BoardApproval } from './board-api'
import { approvalArgsText, isGuardReason } from './card-presentation'

export const APPROVAL_HOLD_MS = 600

// approval 是渲染时从看板最新数据解析出的那条审批：详情晚到时确认层随之更新，
// 在别处处理掉时由控制器关闭确认层；null 表示详情还没到，此时不能批准。
export type ApprovalConfirmTarget = {
  sessionId: string
  title: string
  cwd: string
  approval: BoardApproval | null
}

type ApprovalConfirmProps = {
  target: ApprovalConfirmTarget | null
  phone: boolean
  pending: boolean
  error: string
  onConfirm: () => void
  onOpenChange: (open: boolean) => void
}

// 按住确认：按下开始计时并播放进度条，松开或移出即取消；到时只触发一次。
function HoldToConfirm({
  disabled,
  onConfirm,
  label,
  className,
}: {
  disabled: boolean
  onConfirm: () => void
  label: string
  className?: string
}) {
  const [holding, setHolding] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const cancel = () => {
    if (timer.current) clearTimeout(timer.current)
    timer.current = null
    setHolding(false)
  }
  useEffect(() => cancel, [])
  const start = (event: PointerEvent<HTMLButtonElement>) => {
    if (disabled || event.button !== 0) return
    event.currentTarget.setPointerCapture?.(event.pointerId)
    setHolding(true)
    timer.current = setTimeout(() => {
      timer.current = null
      setHolding(false)
      onConfirm()
    }, APPROVAL_HOLD_MS)
  }
  return (
    <Button
      type="button"
      variant="default"
      disabled={disabled}
      // 手机上按住是批准 critical 的唯一方式：禁止触摸滚动、长按菜单与系统长按气泡打断按住
      // （Android 长按阈值约 500ms，比 600ms 短），配合 pointer capture 让轻微移动也不取消。
      className={cn(
        'relative touch-none overflow-hidden select-none [-webkit-touch-callout:none]',
        className,
      )}
      onPointerDown={start}
      onPointerUp={cancel}
      onPointerCancel={cancel}
      onPointerLeave={cancel}
      onContextMenu={(event) => event.preventDefault()}
      data-approval-hold
      // 键盘用户在确认层内按 ↵ 确认（层级处理），这里不让空格/回车绕过按住时长。
      onClick={(event) => event.preventDefault()}
      aria-describedby="approval-hold-hint"
    >
      <span
        aria-hidden="true"
        className={cn(
          'pointer-events-none absolute inset-0 origin-left scale-x-0 bg-on-accent/20',
          holding && 'scale-x-100 transition-transform duration-[600ms] ease-linear',
        )}
      />
      <span className="relative">{label}</span>
    </Button>
  )
}

export function ApprovalConfirm({
  target,
  phone,
  pending,
  error,
  onConfirm,
  onOpenChange,
}: ApprovalConfirmProps) {
  const { t } = useI18n()
  const open = Boolean(target)
  // 关闭时的退场动画里继续显示最后一次的内容，不闪成“正在读取审批详情”的空态。
  const [lastTarget, setLastTarget] = useState(target)
  if (target && target !== lastTarget) setLastTarget(target)
  const shown = target ?? lastTarget
  const approval = shown?.approval ?? null
  const detail = approvalArgsText(approval)
  const critical = approval?.severity === 'critical'
  // 文案按事实区分：详情未到时只说正在读取；守卫拦截写守卫原因；其他高风险与标准审批各自说明。
  const title = critical ? t('board:confirm.title') : t('board:confirm.titleStandard')
  const description = !approval
    ? t('board:confirm.descriptionLoading')
    : critical
      ? isGuardReason(approval.reason)
        ? t('board:confirm.description')
        : t('board:confirm.descriptionCritical')
      : t('board:confirm.descriptionStandard')
  const keyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (
      event.defaultPrevented ||
      event.key !== 'Enter' ||
      event.nativeEvent.isComposing ||
      pending ||
      !approval
    )
      return
    // 焦点在“取消”上时回车只取消，避免误批准。
    if ((event.target as HTMLElement).closest('[data-approval-cancel]')) return
    event.preventDefault()
    onConfirm()
  }
  // 打开时焦点落在“按住确认”上：层内按 ↵ 即确认；焦点移到“取消”后 ↵ 只会取消。
  const focusHold = (event: Event) => {
    const root = event.currentTarget instanceof HTMLElement ? event.currentTarget : null
    const hold = root?.querySelector<HTMLElement>('[data-approval-hold]')
    if (!hold) return
    event.preventDefault()
    hold.focus()
  }
  const body = shown && (
    <div className="grid min-w-0 gap-3 text-sm">
      <dl className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs">
        <dt className="text-content-muted">{t('board:confirm.session')}</dt>
        <dd className="min-w-0 truncate font-medium text-content">{shown.title}</dd>
        <dt className="text-content-muted">{t('board:confirm.tool')}</dt>
        <dd className="min-w-0 truncate font-mono text-content">{approval?.toolName || '—'}</dd>
        <dt className="text-content-muted">{t('board:confirm.cwd')}</dt>
        <dd className="min-w-0 font-mono wrap-anywhere text-content-secondary">
          {shown.cwd || '—'}
        </dd>
      </dl>
      {approval?.reason && (
        <p className="flex min-w-0 items-start gap-2 rounded-sm border border-st-attn-border bg-st-attn-soft px-2.5 py-2 text-xs text-st-attn">
          <ShieldAlert className="mt-px size-3.5 flex-none" aria-hidden="true" />
          <span className="min-w-0 break-words">{approval.reason}</span>
        </p>
      )}
      {approval ? (
        <pre
          aria-label={t('board:confirm.fullArgs')}
          className="max-h-56 min-w-0 overflow-auto rounded-sm border border-well-stroke bg-well px-2.5 py-2 font-mono text-xs leading-5 whitespace-pre-wrap wrap-anywhere text-well-text"
        >
          {detail}
        </pre>
      ) : (
        <p role="status" className="text-xs text-content-muted">
          {t('board:confirm.loading')}
        </p>
      )}
      {error && (
        <p role="alert" className="text-xs text-st-fail">
          {error}
        </p>
      )}
      {/* 触屏没有回车：手机只提示按住。 */}
      <p id="approval-hold-hint" className="flex items-center gap-1.5 text-xs text-content-muted">
        {phone ? (
          t('board:confirm.hintTouch')
        ) : (
          <>
            {t('board:confirm.hint')}
            <Kbd>↵</Kbd>
          </>
        )}
      </p>
    </div>
  )
  const actions = (
    <>
      <Button
        variant="outline"
        size="dialogAction"
        data-approval-cancel
        className={cn(phone && 'min-h-11 flex-1')}
        onClick={() => onOpenChange(false)}
      >
        {t('board:confirm.cancel')}
      </Button>
      <HoldToConfirm
        disabled={pending || !approval}
        label={pending ? t('board:confirm.approving') : t('board:confirm.hold')}
        onConfirm={onConfirm}
        className={cn('min-h-9 px-4', phone && 'min-h-11 flex-1')}
      />
    </>
  )
  if (phone)
    return (
      <Sheet open={open} onOpenChange={onOpenChange}>
        <SheetContent
          side="bottom"
          showCloseButton={false}
          className="max-h-[85dvh] gap-3 overflow-y-auto rounded-t-(--r-sheet) px-4 pt-2 pb-[calc(16px+var(--pisper-safe-area-bottom))]"
          onKeyDown={keyDown}
          onOpenAutoFocus={focusHold}
        >
          <span
            aria-hidden="true"
            className="mx-auto h-1 w-9 flex-none rounded-full bg-stroke-strong"
          />
          <SheetHeader className="p-0">
            <SheetTitle>{title}</SheetTitle>
            <SheetDescription>{description}</SheetDescription>
          </SheetHeader>
          {body}
          <div className="flex gap-2">{actions}</div>
        </SheetContent>
      </Sheet>
    )
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="wide" onKeyDown={keyDown} onOpenAutoFocus={focusHold}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        {body}
        <DialogFooter>{actions}</DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
