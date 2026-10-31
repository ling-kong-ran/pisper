// 桌面是锚定弹层、手机外壳是底部表单（规格 v2 第 7.2 节：输入框芯片“点开各自的底部表单”）。
// 同一份内容两种外形：调用方给出触发键与内容，sheet 为真时换成 Sheet（带拖拽条与标题），
// Esc、Android 返回键（BackNavigation 先关弹层）与点遮罩都能关上；关闭后焦点回到触发键。
// open / onOpenChange 可选：不传时组件自己持有开关状态（与 Radix Popover 的非受控用法一致）。
import { useState, type ReactElement, type ReactNode } from 'react'
import { Slot } from 'radix-ui'
import { SheetDragHandle } from '@/components/app/SheetDragHandle'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { cn } from '@/lib/utils'

type PopoverSheetProps = {
  sheet: boolean
  // 底部表单的标题（读屏与视觉都用）；弹层形态不显示。
  title: string
  trigger: ReactElement
  children: ReactNode
  open?: boolean
  onOpenChange?: (open: boolean) => void
  align?: 'start' | 'center' | 'end'
  side?: 'top' | 'bottom' | 'left' | 'right'
  sideOffset?: number
  className?: string
  sheetClassName?: string
}

export function PopoverSheet({
  sheet,
  title,
  trigger,
  children,
  open: controlledOpen,
  onOpenChange,
  align,
  side,
  sideOffset,
  className,
  sheetClassName,
}: PopoverSheetProps) {
  const [ownOpen, setOwnOpen] = useState(false)
  const open = controlledOpen ?? ownOpen
  const setOpen = (next: boolean) => {
    if (controlledOpen === undefined) setOwnOpen(next)
    onOpenChange?.(next)
  }
  if (!sheet)
    return (
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>{trigger}</PopoverTrigger>
        <PopoverContent align={align} side={side} sideOffset={sideOffset} className={className}>
          {children}
        </PopoverContent>
      </Popover>
    )
  return (
    <>
      <Slot.Root aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(true)}>
        {trigger}
      </Slot.Root>
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetContent
          side="bottom"
          showCloseButton={false}
          data-popover-sheet
          className={cn(
            'max-h-[85dvh] gap-1 overflow-y-auto rounded-t-(--r-sheet) px-3 pt-1 pb-[calc(12px+var(--pisper-safe-area-bottom))]',
            sheetClassName,
          )}
        >
          <SheetDragHandle onDismiss={() => setOpen(false)} />
          <SheetTitle className="px-2 pt-1 pb-1 text-base font-semibold">{title}</SheetTitle>
          <SheetDescription className="sr-only">{title}</SheetDescription>
          {children}
        </SheetContent>
      </Sheet>
    </>
  )
}
