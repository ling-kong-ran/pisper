import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'

const widths = {
  workbench: 'h-full max-w-none',
  'list-detail': 'flex h-full max-w-none flex-col',
  collection: 'w-full max-w-none p-4 @min-[760px]/page:p-6',
  settings: 'w-full max-w-[776px] px-4 pb-6 @min-[760px]/page:px-6',
  'settings-wide': 'w-full max-w-[1040px] px-4 pb-6 @min-[760px]/page:px-6',
} as const

// 默认整页滚动；内容模式把高度约束交给内部列表，工具栏不随结果滚走。
export function PageFrame({
  variant,
  embedded = false,
  scrollMode = 'page',
  className,
  children,
  ...props
}: ComponentProps<'div'> & {
  variant: keyof typeof widths
  embedded?: boolean
  scrollMode?: 'page' | 'content'
}) {
  if (embedded)
    return (
      <div
        {...props}
        className={cn(
          'min-w-0',
          scrollMode === 'content' && 'flex h-full min-h-0 flex-col overflow-hidden',
          className,
        )}
      >
        {children}
      </div>
    )
  return (
    <div
      data-page-template={variant}
      className={cn(
        '@container/page h-full min-h-0 min-w-0',
        scrollMode === 'content' || variant === 'workbench' || variant === 'list-detail'
          ? 'overflow-hidden'
          : 'overflow-y-auto overflow-x-hidden',
        className,
      )}
      {...props}
    >
      <div
        className={cn(
          'mx-auto min-w-0',
          widths[variant],
          scrollMode === 'content' && 'flex h-full min-h-0 flex-col overflow-hidden',
        )}
      >
        {children}
      </div>
    </div>
  )
}
