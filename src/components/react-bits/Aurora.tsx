import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'

export function Aurora({ className, ...props }: ComponentProps<'div'>) {
  return (
    <div aria-hidden="true" className={cn('rb-aurora', className)} {...props}>
      <i />
      <i />
      <i />
    </div>
  )
}
