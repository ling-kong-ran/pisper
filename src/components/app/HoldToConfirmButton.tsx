// 按住确认按钮（规格 v2 第 3.5 / 6.6 节）：鼠标、触控按住，或键盘按住空格 / 回车，满 600ms 才触发；
// 中途松开 200ms 退回。点击本身不确认——辅助技术直接派发的点击（没有按住过程，event.detail 为 0）
// 交给 onActivate，由调用方改走确认对话框，读屏用户不必“按住”。
// 无业务语义：批准、删除等危险操作都可以用；调用方负责在减少动效时换成对话框。
import { useEffect, useRef, useState, type ComponentProps, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

export const HOLD_TO_CONFIRM_MS = 600

type HoldToConfirmButtonProps = Omit<
  ComponentProps<typeof Button>,
  'onClick' | 'onPointerDown' | 'onKeyDown' | 'onKeyUp' | 'children'
> & {
  onConfirm: () => void
  // 没有按住过程的激活（读屏的“双击”、程序化点击）：通常打开确认对话框。
  onActivate?: () => void
  // 回车不计时、交给外层：确认层本身就是第二步，层内按 ↵ 直接确认（ApprovalConfirm），
  // 只有空格走按住。回车的默认点击照样被 onClick 拦下，不会绕过外层的判断。
  enterPassesThrough?: boolean
  children: ReactNode
}

export function HoldToConfirmButton({
  onConfirm,
  onActivate,
  enterPassesThrough = false,
  disabled,
  className,
  children,
  ...props
}: HoldToConfirmButtonProps) {
  const [holding, setHolding] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const cancel = () => {
    if (timer.current) clearTimeout(timer.current)
    timer.current = null
    setHolding(false)
  }
  useEffect(() => cancel, [])
  const start = () => {
    if (disabled || timer.current) return
    setHolding(true)
    timer.current = setTimeout(() => {
      timer.current = null
      setHolding(false)
      onConfirm()
    }, HOLD_TO_CONFIRM_MS)
  }
  const holdKey = (key: string) =>
    (key === ' ' || key === 'Enter') && !(enterPassesThrough && key === 'Enter')
  return (
    <Button
      type="button"
      disabled={disabled}
      // 触控按住时禁止滚动、长按菜单与系统长按气泡打断（Android 长按阈值约 500ms，比 600ms 短），
      // 配合 pointer capture 让轻微移动也不取消。
      className={cn(
        'relative touch-none overflow-hidden select-none [-webkit-touch-callout:none]',
        className,
      )}
      data-holding={holding || undefined}
      {...props}
      onPointerDown={(event) => {
        if (event.button !== 0) return
        event.currentTarget.setPointerCapture?.(event.pointerId)
        start()
      }}
      onPointerUp={cancel}
      onPointerCancel={cancel}
      onPointerLeave={cancel}
      onContextMenu={(event) => event.preventDefault()}
      onKeyDown={(event) => {
        if (!holdKey(event.key)) return
        // 阻止默认动作：回车在按下时、空格在松开时都会合成点击，不能绕过按住时长。
        event.preventDefault()
        if (!event.repeat) start()
      }}
      onKeyUp={(event) => {
        if (!holdKey(event.key)) return
        event.preventDefault()
        cancel()
      }}
      onBlur={cancel}
      onClick={(event) => {
        event.preventDefault()
        if (event.detail === 0 && !holding) onActivate?.()
      }}
    >
      <span
        aria-hidden="true"
        className={cn(
          'pointer-events-none absolute inset-0 origin-left scale-x-0 bg-on-accent/20 transition-transform duration-200 ease-out',
          holding && 'scale-x-100 duration-[600ms] ease-linear',
        )}
      />
      <span className="relative inline-flex items-center gap-1.5">{children}</span>
    </Button>
  )
}
