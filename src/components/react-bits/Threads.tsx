// 装饰动画组件：丝线（threads）背景动效。
import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'
import './react-bits.css'

const THREAD_PATHS = [
  'M-20 18 C120 0 170 76 320 42 S520 2 700 54 S920 104 1100 44',
  'M-30 48 C110 82 200 6 350 48 S560 94 720 44 S930 2 1110 62',
  'M-20 82 C130 42 220 112 390 68 S610 24 760 74 S940 118 1110 72',
  'M-30 112 C120 142 260 72 420 112 S660 154 820 106 S980 62 1120 118',
]

export function Threads({ className, ...props }: ComponentProps<'svg'>) {
  return (
    <svg
      aria-hidden="true"
      className={cn('rb-threads', className)}
      preserveAspectRatio="none"
      viewBox="0 0 1080 140"
      {...props}
    >
      {THREAD_PATHS.map((path, index) => (
        <path d={path} key={path} style={{ animationDelay: `${index * -0.7}s` }} />
      ))}
    </svg>
  )
}
