// AI 元素：消息外壳——按角色（user/assistant/system）提供布局与
// 气泡样式的基础容器。
import type { HTMLAttributes } from 'react'
import { cn } from '@/lib/utils'

export type AiMessageRole = 'user' | 'assistant' | 'system'

export type MessageProps = HTMLAttributes<HTMLDivElement> & {
  from: AiMessageRole
}

// Lightweight Pisper adapter for the AI Elements Message primitive. Rich
// Streamdown rendering remains available in message.tsx and can be loaded only
// when the Pisper protocol exposes a compatible rich-content part.
export function Message({ className, from, ...props }: MessageProps) {
  return (
    <div
      data-ai-element="message"
      data-role={from}
      className={cn(
        'group flex w-full max-w-[95%] flex-col gap-2',
        from === 'user' ? 'is-user ml-auto justify-end' : 'is-assistant',
        className,
      )}
      {...props}
    />
  )
}
