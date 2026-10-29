import { useId, type ComponentProps, type ReactNode } from 'react'
import { cn } from '@/lib/utils'

// 设置组合只接收展示内容与控件，业务状态和持久化由所属页面管理。
// 行与行之间只用细线分隔：分组标题 15/500（600 只留给页面标题，避免中文粗体发重）、
// 行标题 13/500、说明 12px 次要色，控件右对齐。
export function SettingsSection({
  title,
  description,
  action,
  children,
  className,
  ...props
}: Omit<ComponentProps<'section'>, 'title'> & {
  title: ReactNode
  description?: ReactNode
  action?: ReactNode
}) {
  const titleId = useId()
  return (
    <section aria-labelledby={titleId} className={cn('min-w-0', className)} {...props}>
      <div className="mb-3 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 id={titleId} className="text-[15px] leading-6 font-medium text-content">
            {title}
          </h2>
          {description && <p className="mt-0.5 text-[13px] text-content-muted">{description}</p>}
        </div>
        {action}
      </div>
      <div className="divide-y divide-stroke-soft">{children}</div>
    </section>
  )
}

export function SettingsRow({
  title,
  description,
  control,
  nested,
  children,
  className,
  ...props
}: Omit<ComponentProps<'div'>, 'title'> & {
  title: ReactNode
  description?: ReactNode
  control?: ReactNode
  nested?: boolean
}) {
  return (
    <div className={cn('min-w-0 py-3', nested && 'pl-4', className)} {...props}>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-6 gap-y-3">
        <div className="min-w-0 flex-1 basis-40">
          <div className="text-[13px] font-medium text-content">{title}</div>
          {description && (
            <p className="mt-0.5 text-xs leading-relaxed text-content-muted">{description}</p>
          )}
        </div>
        {control && <div className="max-w-full shrink-0">{control}</div>}
      </div>
      {children}
    </div>
  )
}
