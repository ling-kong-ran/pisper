import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { Slot } from 'radix-ui'

import { cn } from '@/lib/utils'

// 芯片：可点的小型选择键（模型、审批模式、Plan 模式、筛选等），28px、r-sm、13/500，
// --solid 面 + --stroke-strong 细边。按下态（aria-pressed）只加深面和边，不用强调色：
// 强调色留给主操作，琥珀留给「需要你」。触屏下常规尺寸抬到 44px 热区。
const chipVariants = cva(
  "inline-flex w-fit max-w-full shrink-0 items-center justify-center gap-1.5 rounded-sm border border-stroke-strong bg-solid font-medium whitespace-nowrap text-content-secondary shadow-edge transition-colors duration-(--d-fast) select-none hover:bg-surface-hover hover:text-foreground disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50 aria-expanded:bg-surface-hover aria-expanded:text-foreground aria-pressed:border-control aria-pressed:bg-surface-active aria-pressed:text-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-3.5 [&_svg:not([class*='text-'])]:text-content-muted",
  {
    variants: {
      size: {
        default: 'h-7 px-2.5 text-sm pointer-coarse:min-h-11',
        sm: 'h-6 gap-1 px-2 text-xs',
      },
    },
    defaultVariants: {
      size: 'default',
    },
  },
)

type ChipProps = React.ComponentProps<'button'> &
  VariantProps<typeof chipVariants> & {
    asChild?: boolean
  }

function Chip({ className, size = 'default', asChild = false, type, ...props }: ChipProps) {
  const Comp = asChild ? Slot.Root : 'button'
  return (
    <Comp
      data-slot="chip"
      data-size={size}
      className={cn(chipVariants({ size }), className)}
      {...(asChild ? {} : { type: type ?? 'button' })}
      {...props}
    />
  )
}

export { Chip, chipVariants }
export type { ChipProps }
