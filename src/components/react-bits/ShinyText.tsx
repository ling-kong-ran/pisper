// 装饰动画组件：金属光泽扫过的文字效果。
import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'
import './react-bits.css'

export function ShinyText({ className, children, ...props }: ComponentProps<'span'>) {
  return (
    <span className={cn('rb-shiny-text', className)} {...props}>
      {children}
    </span>
  )
}
