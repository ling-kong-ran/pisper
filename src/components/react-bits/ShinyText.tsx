import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'

export function ShinyText({ className, children, ...props }: ComponentProps<'span'>) {
  return (
    <span className={cn('rb-shiny-text', className)} {...props}>
      {children}
    </span>
  )
}
