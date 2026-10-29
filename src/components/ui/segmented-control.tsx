import * as React from 'react'
import { ToggleGroup as ToggleGroupPrimitive } from 'radix-ui'

import { cn } from '@/lib/utils'

// 分段控件：单选、不可清空。凹陷轨道（--surface-muted + 1px --stroke）里的选中段凸起为
// --solid 面 + --stroke-strong 内描边，与标题栏主标签同一语言。基于 Radix ToggleGroup：
// 方向键在段之间移动焦点（roving tabindex），每段是 radio 语义。
// 默认 32px 轨道（段 28）对齐按钮高度；sm 为 28px 轨道（段 24）；触屏轨道 44px。
export type SegmentedControlOption<T extends string> = {
  value: T
  label: React.ReactNode
  // 段尾的等宽计数（例如项目下的代理数）；只放机器数字，中文单位由调用方写进 label。
  count?: React.ReactNode
  icon?: React.ReactNode
  disabled?: boolean
  // 只有图标、没有可见文字的段必须提供，供读屏读出。
  ariaLabel?: string
}

type SegmentedControlProps<T extends string> = Omit<
  React.ComponentProps<'div'>,
  'defaultValue' | 'dir' | 'onChange'
> & {
  value: T
  onValueChange: (value: T) => void
  options: readonly SegmentedControlOption<T>[]
  size?: 'default' | 'sm'
  disabled?: boolean
}

const TRACK_SIZES = {
  default: 'h-8 pointer-coarse:h-11',
  sm: 'h-7',
} as const

const ITEM_SIZES = {
  default: 'h-7 gap-1.5 px-2.5 text-sm pointer-coarse:h-10',
  sm: 'h-6 gap-1 px-2 text-xs',
} as const

function SegmentedControl<T extends string>({
  value,
  onValueChange,
  options,
  size = 'default',
  disabled = false,
  className,
  ...props
}: SegmentedControlProps<T>) {
  return (
    <ToggleGroupPrimitive.Root
      type="single"
      // Radix 单选组默认 role=group；分段控件语义上是一组互斥选项。
      role="radiogroup"
      data-slot="segmented-control"
      data-size={size}
      value={value}
      disabled={disabled}
      // Radix 单选允许再次点击取消选中；分段控件必须始终有一个值，空值直接忽略。
      onValueChange={(next) => {
        if (next) onValueChange(next as T)
      }}
      className={cn(
        'inline-flex w-fit max-w-full items-center gap-0.5 rounded-md bg-surface-muted p-0.5 shadow-[inset_0_0_0_1px_var(--stroke)]',
        TRACK_SIZES[size],
        className,
      )}
      {...props}
    >
      {options.map((option) => (
        <ToggleGroupPrimitive.Item
          key={option.value}
          value={option.value}
          disabled={option.disabled}
          aria-label={option.ariaLabel}
          data-slot="segmented-control-item"
          className={cn(
            "inline-flex min-w-0 shrink-0 items-center justify-center rounded-sm font-medium whitespace-nowrap text-content-secondary transition-colors duration-(--d-fast) select-none hover:text-foreground focus-visible:z-10 disabled:pointer-events-none disabled:opacity-50 data-[state=on]:bg-solid data-[state=on]:text-foreground data-[state=on]:shadow-[inset_0_0_0_1px_var(--stroke-strong),var(--sh-1)] [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-3.5",
            ITEM_SIZES[size],
          )}
        >
          {option.icon}
          {option.label}
          {option.count !== undefined && option.count !== null && (
            <span className="font-mono text-xs text-content-muted tabular-nums">
              {option.count}
            </span>
          )}
        </ToggleGroupPrimitive.Item>
      ))}
    </ToggleGroupPrimitive.Root>
  )
}

export { SegmentedControl }
export type { SegmentedControlProps }
