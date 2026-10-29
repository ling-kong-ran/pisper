import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { StarOrbit } from '@/components/StarOrbit'
import { OrbitSpinner } from '@/components/ui/spinner'
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
      className="flex min-h-64 w-full min-w-0 flex-col items-center justify-center gap-2 px-4 py-8 text-center"
    >
      {/* 加载沿用品牌轨道指示；空状态给出星轨插画，错误不加装饰以免冲淡告警。 */}
      {kind === 'loading' && <OrbitSpinner className="mb-1 size-5" />}
      {kind === 'empty' && <StarOrbit className="mb-1" />}
      <h2 className="text-sm font-medium text-content">{title}</h2>
      {description && (
        <p className="max-w-md text-xs leading-relaxed break-words text-content-muted">
          {description}
        </p>
      )}
      {action && <div className="mt-2">{action}</div>}
    </div>
  )
}

// 卡片内的空状态：1px 虚线卡片 + 星轨插画（或调用方给出的语义图标），14/500 标题与 12px 说明。
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
        'grid place-content-center justify-items-center gap-1 px-6 py-6 text-center text-content-muted',
        // 紧凑态嵌在已有卡片/列表里，不再画第二层边框；独立空状态才是虚线卡片。
        compact
          ? 'min-h-[110px] py-4'
          : 'min-h-[185px] rounded-xl border border-dashed border-stroke-strong',
        className,
      )}
    >
      {illustration ??
        (Icon ? (
          <span className="mb-1.5 grid size-8 place-items-center rounded-md bg-key text-content-secondary shadow-key ring-1 ring-key-border">
            <Icon className="size-4" aria-hidden="true" />
          </span>
        ) : (
          <StarOrbit className="mb-1" />
        ))}
      <strong className="text-sm font-medium text-content-secondary">{title}</strong>
      {description && <p className="max-w-[340px] text-xs leading-relaxed">{description}</p>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  )
}
