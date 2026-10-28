import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { LoaderCircle } from 'lucide-react'
import { cn } from '@/lib/utils'

export function PageState({
  kind,
  title,
  description,
  action,
}: {
  kind: 'loading' | 'empty' | 'error'
  title: ReactNode
  description?: ReactNode
  action?: ReactNode
}) {
  return (
    <div
      role={kind === 'error' ? 'alert' : 'status'}
      aria-busy={kind === 'loading'}
      className="flex min-h-64 w-full min-w-0 flex-col items-center justify-center gap-3 px-4 py-8 text-center"
    >
      {kind === 'loading' && (
        <LoaderCircle className="size-5 animate-spin text-content-muted" aria-hidden="true" />
      )}
      <h2 className="text-base font-medium text-content">{title}</h2>
      {description && (
        <p className="max-w-md break-words text-sm text-content-muted">{description}</p>
      )}
      {action}
    </div>
  )
}

// 卡片内的空状态：比整页 PageState 紧凑，图标统一放在浅色圆底里；需要品牌插图时传 illustration。
export function InlineEmptyState({
  icon: Icon,
  illustration,
  title,
  description,
  action,
  compact = false,
  className,
}: {
  icon?: LucideIcon
  illustration?: ReactNode
  title: ReactNode
  description?: ReactNode
  action?: ReactNode
  compact?: boolean
  className?: string
}) {
  return (
    <div
      data-slot="inline-empty-state"
      className={cn(
        'grid place-content-center justify-items-center gap-1 px-6 py-6 text-center text-muted-foreground',
        compact ? 'min-h-[110px]' : 'min-h-[185px]',
        className,
      )}
    >
      {illustration ??
        (Icon && (
          <span className="mb-1.5 grid size-10 place-items-center rounded-full bg-[var(--surface-subtle)]">
            <Icon className="size-[18px]" aria-hidden="true" />
          </span>
        ))}
      <strong className="text-[13px] font-medium text-foreground">{title}</strong>
      {description && <p className="max-w-[340px] text-xs leading-relaxed">{description}</p>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  )
}
