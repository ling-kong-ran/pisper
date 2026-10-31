// 设置栏与手机详情共用的小节外框：标题 + 右侧说明 + 内容。单独成文件，好让版本、能力等小节文件
// 不必反向导入 StudioSettings（会形成依赖环）。
import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'

export function StudioSection({
  title,
  aside,
  children,
  className,
}: {
  title: ReactNode
  aside?: ReactNode
  children: ReactNode
  className?: string
}) {
  return (
    <section className={cn('flex flex-col gap-2.5', className)}>
      <header className="flex items-baseline justify-between gap-2">
        <h3 className="text-sm font-semibold text-content">{title}</h3>
        {aside && <span className="text-xs text-content-muted">{aside}</span>}
      </header>
      {children}
    </section>
  )
}
