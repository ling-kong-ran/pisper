// 装饰动画组件：极光渐变背景。
import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'
import './react-bits.css'

export function Aurora({ className, ...props }: ComponentProps<'div'>) {
  return (
    <div
      aria-hidden="true"
      className={cn(
        "rb-aurora [.agent-welcome_>_:not(&)]:relative [.agent-welcome_>_:not(&)]:z-[1] [:root[data-theme='light']_.agent-welcome_&_i]:opacity-[.1] [:root[data-theme='light']_.agent-welcome_&_i:nth-child(1)]:text-[#60a5fa] [:root[data-theme='light']_.agent-welcome_&_i:nth-child(1)]:opacity-[.14] [:root[data-theme='light']_.agent-welcome_&_i:nth-child(2)]:text-[#a78bfa] [:root[data-theme='light']_.agent-welcome_&_i:nth-child(2)]:opacity-[.1] [:root[data-theme='light']_.agent-welcome_&_i:nth-child(3)]:text-[#5eead4] [:root[data-theme='light']_.agent-welcome_&_i:nth-child(3)]:opacity-[.09]",
        className,
      )}
      {...props}
    >
      <i />
      <i />
      <i />
    </div>
  )
}
