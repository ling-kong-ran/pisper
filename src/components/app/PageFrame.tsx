import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'

// 设置类页面最大 1040px、左右 40px 留白，与页头 20/600 标题左缘对齐（标题在页头里，桌面不加顶部留白；
// 手机隐藏大标题，改由内容自带 16px 顶部留白）；
// 集合页铺满纸面宽度，28/40 的留白与设置页保持同一节奏。
const widths = {
  workbench: 'h-full max-w-none',
  'list-detail': 'flex h-full max-w-none flex-col',
  collection: 'w-full max-w-none p-4 @min-[760px]/page:px-10 @min-[760px]/page:py-7',
  settings: 'w-full max-w-[776px] px-4 pb-8 max-[650px]:pt-4 @min-[760px]/page:px-10',
  'settings-wide': 'w-full max-w-[1040px] px-4 pb-8 max-[650px]:pt-4 @min-[760px]/page:px-10',
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
