import * as React from 'react'

import { cn } from '@/lib/utils'

// 34px 字段（触屏 44px 热区）：控件边框 ≥3:1；聚焦时边框换为 --focus 并加 3px 焦点光环，插入符同为强调色。
function Input({ className, type, ...props }: React.ComponentProps<'input'>) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        'h-8.5 pointer-coarse:h-11 w-full min-w-0 rounded-sm border border-input bg-panel px-2.5 py-1 text-base caret-focus transition-[border-color,box-shadow] outline-none file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground focus-visible:border-focus focus-visible:ring-3 focus-visible:ring-focus-ring disabled:pointer-events-none disabled:cursor-not-allowed disabled:bg-surface-muted disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 md:text-sm',
        className,
      )}
      {...props}
    />
  )
}

export { Input }
