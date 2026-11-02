import * as React from 'react'
import { Popover as PopoverPrimitive } from 'radix-ui'

import { cn } from '@/lib/utils'
import { DialogPortalContainerContext } from '@/components/ui/dialog'

function Popover({ modal = false, ...props }: React.ComponentProps<typeof PopoverPrimitive.Root>) {
  return <PopoverPrimitive.Root data-slot="popover" modal={modal} {...props} />
}

function PopoverTrigger({ ...props }: React.ComponentProps<typeof PopoverPrimitive.Trigger>) {
  return <PopoverPrimitive.Trigger data-slot="popover-trigger" {...props} />
}

// 弹层最高不超过 Radix 算出的可用高度（视口减去触发键与碰撞边距），超出时在弹层内滚动：
// 内容再长也不会顶出窗口（例如模型芯片弹层展开“详细统计”时）。
function PopoverContent({
  className,
  align = 'center',
  sideOffset = 8,
  alignOffset = align === 'center' ? 0 : -5,
  collisionPadding = 20,
  portalContainer,
  ...props
}: React.ComponentProps<typeof PopoverPrimitive.Content> & {
  portalContainer?: HTMLElement | null
}) {
  const dialogContainer = React.useContext(DialogPortalContainerContext)
  return (
    <PopoverPrimitive.Portal
      container={portalContainer === undefined ? (dialogContainer ?? undefined) : portalContainer}
    >
      <PopoverPrimitive.Content
        data-slot="popover-content"
        align={align}
        sideOffset={sideOffset}
        alignOffset={alignOffset}
        collisionPadding={collisionPadding}
        className={cn(
          'z-[200] flex max-h-(--radix-popover-content-available-height) w-72 max-w-[calc(100vw-40px)] origin-(--radix-popover-content-transform-origin) flex-col gap-2.5 overflow-y-auto overscroll-contain rounded-md bg-popover p-2.5 text-sm text-popover-foreground shadow-float outline-hidden duration-100 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0',
          className,
        )}
        {...props}
      />
    </PopoverPrimitive.Portal>
  )
}

function PopoverAnchor({ ...props }: React.ComponentProps<typeof PopoverPrimitive.Anchor>) {
  return <PopoverPrimitive.Anchor data-slot="popover-anchor" {...props} />
}

function PopoverHeader({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="popover-header"
      className={cn('flex flex-col gap-0.5 text-sm', className)}
      {...props}
    />
  )
}

function PopoverTitle({ className, ...props }: React.ComponentProps<'h2'>) {
  return <div data-slot="popover-title" className={cn('font-medium', className)} {...props} />
}

function PopoverDescription({ className, ...props }: React.ComponentProps<'p'>) {
  return (
    <p
      data-slot="popover-description"
      className={cn('text-muted-foreground', className)}
      {...props}
    />
  )
}

export {
  Popover,
  PopoverAnchor,
  PopoverContent,
  PopoverDescription,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
}
