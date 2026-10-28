import { useId, type ComponentProps, type ReactNode } from 'react'
import { cn } from '@/lib/utils'

// 设置组合只接收展示内容与控件，业务状态和持久化由所属页面管理。
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
          <h2 id={titleId} className="text-lg font-medium">
            {title}
          </h2>
          {description && <p className="mt-1 text-sm text-muted-foreground">{description}</p>}
        </div>
        {action}
      </div>
      <div className="divide-y divide-border">{children}</div>
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
          <div className="text-sm font-medium">{title}</div>
          {description && (
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{description}</p>
          )}
        </div>
        {control && <div className="max-w-full shrink-0">{control}</div>}
      </div>
      {children}
    </div>
  )
}
