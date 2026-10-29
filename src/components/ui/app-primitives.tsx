// 应用级 UI 原语：AppCard（卡片 + 徽标 + 描述）、AppEmptyState（空态）
// 等跨功能页复用的组合组件。
import { useEffect, useRef, useState, type ComponentProps, type ReactNode } from 'react'
import { Badge } from '@/components/ui/badge'
import { Card, CardTitle } from '@/components/ui/card'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { cn } from '@/lib/utils'

// 状态徽标只用软底 + 语义文字色；灰色档带细描边，与正文区分但不抢视线。
const STATUS_BADGE_TONES = {
  blue: 'bg-info-soft text-info',
  green: 'bg-success-soft text-success-strong',
  red: 'bg-danger-soft text-danger',
  amber: 'bg-warning-soft text-warning-strong',
  gray: 'border-stroke bg-surface-muted text-content-secondary',
} as const

export type StatusBadgeTone = keyof typeof STATUS_BADGE_TONES

export function AppCard({ className, ...props }: ComponentProps<typeof Card>) {
  return (
    <Card
      className={cn(
        'app-card [&_h2]:text-base [&_h2]:tracking-normal shrink-0 block min-w-0 gap-0 overflow-visible rounded-md border border-border bg-card p-3.5 py-3.5 text-card-foreground shadow-none ring-0',
        className,
      )}
      {...props}
    />
  )
}

export function AppCardHeader({ className, ...props }: ComponentProps<'div'>) {
  return (
    <div
      data-slot="app-card-header"
      className={cn(
        'flex min-w-0 items-start justify-between gap-3 [&_h2]:text-base [&_h2]:tracking-[0] [&_h3]:text-sm [&_p]:mt-1 [&_p]:text-xs [&_p]:text-content-muted [&_span]:text-xs [&_span]:text-content-muted [&_a]:text-xs [&_a]:font-semibold [&_a]:text-content-soft [&_a]:no-underline hover:[&_a]:text-content',
        className,
      )}
      {...props}
    />
  )
}

export function AppEmptyState({ className, ...props }: ComponentProps<typeof AppCard>) {
  return (
    <AppCard
      data-slot="app-empty-state"
      className={cn(
        'col-span-full grid min-h-[300px] place-content-center justify-items-center rounded-xl border-dashed border-stroke-strong bg-transparent text-content-muted [&>svg:first-child:not(.animate-spin)]:box-content! [&>svg:first-child:not(.animate-spin)]:rounded-full [&>svg:first-child:not(.animate-spin)]:border [&>svg:first-child:not(.animate-spin)]:border-dashed [&>svg:first-child:not(.animate-spin)]:border-stroke-strong [&>svg:first-child:not(.animate-spin)]:p-4 [&_h2]:mt-4 [&_h2]:text-base [&_h2]:font-medium [&_h2]:text-content-secondary [&_p]:mt-1 [&_p]:max-w-[360px] [&_p]:text-center [&_p]:text-xs [&_p]:leading-[1.6]',
        className,
      )}
      {...props}
    />
  )
}

export function AppError({ className, ...props }: ComponentProps<'div'>) {
  return (
    <div
      role="alert"
      data-slot="app-error"
      className={cn(
        'my-2.5 flex items-start gap-1.5 rounded-sm bg-danger-soft p-2 text-sm leading-[1.4] text-danger',
        className,
      )}
      {...props}
    />
  )
}

export function AppNotice({ className, ...props }: ComponentProps<'div'>) {
  return (
    <div
      data-slot="app-notice"
      className={cn(
        'mt-3 flex gap-2 rounded-sm bg-info-soft p-2.5 text-info [&>span]:flex [&>span]:flex-col [&>span]:gap-[3px] [&_strong]:text-sm [&_strong]:font-medium [&_small]:text-sm [&_small]:leading-[1.4] [&_small]:text-content-muted',
        className,
      )}
      {...props}
    />
  )
}

export function AppSectionTitle({ title, className }: { title: ReactNode; className?: string }) {
  return (
    <CardTitle
      role="heading"
      aria-level={3}
      className={cn(
        'app-section-title text-sm leading-5 font-medium tracking-normal text-content-soft',
        className,
      )}
    >
      {title}
    </CardTitle>
  )
}

export function StatusBadge({
  children,
  tone = 'blue',
  className,
  ...props
}: Omit<ComponentProps<typeof Badge>, 'variant'> & { tone?: StatusBadgeTone }) {
  return (
    <Badge
      variant="secondary"
      className={cn(
        'status-badge h-auto min-h-5 rounded-xs border-transparent px-1.5 text-xs leading-4 font-medium tracking-[0]',
        STATUS_BADGE_TONES[tone],
        className,
      )}
      {...props}
    >
      {children}
    </Badge>
  )
}

export function AppSwitch({
  defaultOn = false,
  value,
  onChange,
  ariaLabel,
  density = 'default',
  className,
  onClick,
  ...props
}: Omit<
  ComponentProps<typeof Switch>,
  'aria-label' | 'checked' | 'defaultChecked' | 'onChange' | 'onCheckedChange' | 'value'
> & {
  defaultOn?: boolean
  value?: boolean
  onChange?: (value: boolean) => void
  ariaLabel?: string
  density?: 'default' | 'compact'
}) {
  const [internal, setInternal] = useState(defaultOn)
  const checked = value ?? internal

  return (
    <Switch
      checked={checked}
      onCheckedChange={(next) => {
        if (value === undefined) setInternal(next)
        onChange?.(next)
      }}
      aria-label={ariaLabel}
      // 两种密度共用 34×20 设计开关；密度参数保留给调用方，热区由开关自身扩展。
      data-switch-density={density}
      className={cn('app-switch', className)}
      onClick={(event) => {
        event.stopPropagation()
        onClick?.(event)
      }}
      {...props}
    />
  )
}

export function SegmentedTabs({
  options,
  value,
  onChange,
  compact = false,
  className,
}: {
  options: string[]
  value: string
  onChange: (value: string) => void
  compact?: boolean
  className?: string
}) {
  const listRef = useRef<HTMLDivElement>(null)
  const [fade, setFade] = useState<'start' | 'end' | 'both' | undefined>()
  const optionKey = options.join('\u0000')
  // 轨道隐藏滚动条，选项放不下时只靠截断的分段很难看出可滚动：按实际溢出方向给对应
  // 端加淡出遮罩。尺寸或选项文案变化（含切换语言）都会重新测量。
  useEffect(() => {
    const list = listRef.current
    if (!list || typeof ResizeObserver !== 'function') return
    const update = () => {
      const max = list.scrollWidth - list.clientWidth
      const start = max > 1 && list.scrollLeft > 1
      const end = max > 1 && list.scrollLeft < max - 1
      setFade(start && end ? 'both' : start ? 'start' : end ? 'end' : undefined)
    }
    update()
    const observer = new ResizeObserver(update)
    observer.observe(list)
    list.addEventListener('scroll', update, { passive: true })
    return () => {
      observer.disconnect()
      list.removeEventListener('scroll', update)
    }
  }, [optionKey])

  return (
    <Tabs value={value} onValueChange={onChange} className={cn('block min-w-0', className)}>
      {/* 手机与触屏上是 44px 轨道（分段 36px 键帽）；≤650px 轨道铺满可用宽度、分段均分剩余空间。
          轨道会裁切溢出，焦点描边因此内收，避免被裁掉一半。 */}
      <TabsList
        ref={listRef}
        data-fade={fade}
        className={cn(
          'segmented min-h-9 max-w-full justify-start gap-0.5 overflow-x-auto overflow-y-hidden scroll-px-3 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden pointer-coarse:min-h-11 max-[650px]:min-h-11 max-[650px]:w-full',
          'data-[fade=both]:[mask-image:linear-gradient(to_right,transparent,var(--foreground)_1.5rem,var(--foreground)_calc(100%-1.5rem),transparent)] data-[fade=end]:[mask-image:linear-gradient(to_right,var(--foreground)_calc(100%-1.5rem),transparent)] data-[fade=start]:[mask-image:linear-gradient(to_left,var(--foreground)_calc(100%-1.5rem),transparent)]',
          compact && 'compact w-auto',
        )}
      >
        {options.map((option) => (
          <TabsTrigger
            className="min-w-14 flex-none px-3 tracking-[0] focus-visible:-outline-offset-2 max-[650px]:min-h-9 max-[650px]:grow max-[650px]:text-base"
            value={option}
            key={option}
            // Chromium 聚焦只在目标完全不可见时才滚动；被淡出端遮住大半的分段需要显式滚入轨道。
            onFocus={(event) =>
              event.currentTarget.scrollIntoView({ block: 'nearest', inline: 'nearest' })
            }
          >
            {option}
          </TabsTrigger>
        ))}
      </TabsList>
    </Tabs>
  )
}
