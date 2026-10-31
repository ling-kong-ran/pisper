import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'

// 页面外框：只剩全宽工作台（工作流节点编辑器、我的组件整页）与组件工作室的集合页在用。
// 功能页与设置分区改用功能页模板（src/components/app/FeaturePage.tsx，A6），原先的 settings / settings-wide /
// list-detail 三种宽度随之退役。集合页铺满纸面宽度，28/40 的留白。
const widths = {
  workbench: 'h-full max-w-none',
  collection: 'w-full max-w-none p-4 @min-[760px]/page:px-10 @min-[760px]/page:py-7',
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
        scrollMode === 'content' || variant === 'workbench'
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
