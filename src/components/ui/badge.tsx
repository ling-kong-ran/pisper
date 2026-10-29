import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { Slot } from 'radix-ui'

import { cn } from '@/lib/utils'

// 徽标：20px、r-xs、12/500。neutral 为默认；accent 用于「默认」等强调标记，
// ok/bad/warn 对应成功、失败、待审批，run 为运行中（调用方放入轨道指示）。
const badgeVariants = cva(
  'group/badge inline-flex h-5 w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-xs border border-transparent px-1.5 text-xs leading-none font-medium whitespace-nowrap tabular-nums transition-colors has-data-[icon=inline-end]:pr-1 has-data-[icon=inline-start]:pl-1 aria-invalid:border-destructive [&>svg]:pointer-events-none [&>svg]:size-3!',
  {
    variants: {
      variant: {
        default: 'border-stroke bg-surface-muted text-content-secondary',
        secondary:
          'border-stroke bg-surface-muted text-content-secondary [a]:hover:bg-surface-hover',
        accent: 'border-star-border bg-star-soft text-star-strong',
        ok: 'bg-success-soft text-success',
        bad: 'bg-danger-soft text-danger',
        warn: 'bg-warning-soft text-warning-strong',
        run: 'bg-star-soft text-accent-fg',
        destructive: 'bg-danger-soft text-danger',
        outline: 'border-stroke text-foreground [a]:hover:bg-surface-hover',
        ghost: 'hover:bg-surface-hover',
        link: 'text-brand underline-offset-4 hover:underline',
      },
    },
    defaultVariants: {
      variant: 'default',
    },
  },
)

function Badge({
  className,
  variant = 'default',
  asChild = false,
  ...props
}: React.ComponentProps<'span'> & VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : 'span'

  return (
    <Comp
      data-slot="badge"
      data-variant={variant}
      className={cn(badgeVariants({ variant }), className)}
      {...props}
    />
  )
}

export { Badge, badgeVariants }
