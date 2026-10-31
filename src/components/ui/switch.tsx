'use client'

import * as React from 'react'
import { Switch as SwitchPrimitive } from 'radix-ui'

import { cn } from '@/lib/utils'

// 34×20 开关：关闭为凹陷轨道 + 控件边框（≥3:1）+ 灰色滑块，开启为强调色轨道 + 反色滑块；
// 滑块位移 180ms。焦点沿用 base 层 2px 外描边；after 伪元素把触控热区扩到 44px 高
// （伪元素按内边距盒定位：20 高减去上下 1px 边框是 18，上下各扩 13 才是 44，原来各扩 12 只有 42）。
function Switch({
  className,
  size = 'default',
  ...props
}: React.ComponentProps<typeof SwitchPrimitive.Root> & {
  size?: 'sm' | 'default'
}) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      data-size={size}
      className={cn(
        'peer group/switch relative inline-flex min-h-0! shrink-0 items-center rounded-full border border-control bg-surface-active transition-colors duration-(--d-fast) after:absolute after:-inset-x-3 after:-inset-y-[13px] aria-invalid:border-destructive data-[size=default]:h-5 data-[size=default]:w-[34px] data-[size=sm]:h-4 data-[size=sm]:w-[26px] data-checked:border-star data-checked:bg-star data-disabled:cursor-not-allowed data-disabled:opacity-50',
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className="pointer-events-none block translate-x-0.5 rounded-full bg-content-muted transition-transform duration-(--d-base) ease-(--ease) group-data-[size=default]/switch:size-3.5 group-data-[size=sm]/switch:size-2.5 data-checked:bg-on-accent data-checked:shadow-xs group-data-[size=default]/switch:data-checked:translate-x-4 group-data-[size=sm]/switch:data-checked:translate-x-3"
      />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
