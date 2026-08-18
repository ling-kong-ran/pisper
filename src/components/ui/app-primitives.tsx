// 应用级 UI 原语：AppCard（卡片 + 徽标 + 描述）、AppEmptyState（空态）
// 等跨功能页复用的组合组件。
import { useState, type ComponentProps, type ReactNode } from 'react'
import { Badge } from '@/components/ui/badge'
import { Card, CardTitle } from '@/components/ui/card'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { cn } from '@/lib/utils'

const STATUS_BADGE_TONES = {
  blue: 'bg-brand-soft text-brand-strong',
  green: 'bg-success-soft text-success-strong',
  red: 'bg-danger-soft text-danger',
  amber: 'bg-warning-soft text-warning-strong',
  gray: 'bg-surface-muted text-content-muted',
} as const

export type StatusBadgeTone = keyof typeof STATUS_BADGE_TONES

export function AppCard({ className, ...props }: ComponentProps<typeof Card>) {
  return (
    <Card
      className={cn(
        'app-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] block min-w-0 gap-0 overflow-visible rounded-surface border border-border bg-card p-3.5 py-3.5 text-card-foreground shadow-surface ring-0 backdrop-blur-[14px]',
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
        'flex min-w-0 items-start justify-between gap-3 [&_h2]:text-base [&_h2]:tracking-[0] [&_h3]:text-sm [&_p]:mt-1 [&_p]:text-[12px] [&_p]:text-content-muted [&_span]:text-[12px] [&_span]:text-content-muted [&_a]:text-[12px] [&_a]:font-semibold [&_a]:text-content-soft [&_a]:no-underline hover:[&_a]:text-content',
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
        'col-span-full grid min-h-[300px] place-content-center justify-items-center text-content-muted [&>svg:first-child:not(.animate-spin)]:box-content [&>svg:first-child:not(.animate-spin)]:rounded-full [&>svg:first-child:not(.animate-spin)]:border [&>svg:first-child:not(.animate-spin)]:border-[var(--stroke-soft)] [&>svg:first-child:not(.animate-spin)]:bg-[radial-gradient(circle_at_50%_38%,var(--surface-subtle),var(--panel)_72%)] [&>svg:first-child:not(.animate-spin)]:p-[26px] [&>svg:first-child:not(.animate-spin)]:text-[var(--text-tertiary)] [&>svg:first-child:not(.animate-spin)]:shadow-[inset_0_1px_0_var(--surface-highlight),0_10px_26px_-18px_var(--shadow)] [&_h2]:mt-[18px] [&_h2]:text-[15px] [&_h2]:text-content [&_p]:mt-[7px] [&_p]:max-w-[360px] [&_p]:text-center [&_p]:text-[13px] [&_p]:leading-[1.6]',
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
        'my-2.5 flex items-start gap-1.5 rounded-[var(--r-xs)] bg-danger-soft p-[7px] text-[13px] leading-[1.4] text-danger',
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
        'mt-3 flex gap-2 rounded-[var(--r-sm)] bg-[var(--accent-soft)] p-[9px] text-[var(--star-strong)] [&>span]:flex [&>span]:flex-col [&>span]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[13px] [&_small]:leading-[1.4] [&_small]:text-content-muted',
        className,
      )}
      {...props}
    />
  )
}

export function AppSectionTitle({ title }: { title: ReactNode }) {
  return (
    <CardTitle
      role="heading"
      aria-level={3}
      className="app-section-title [.selection-list_&]:mb-[8px] [.model-config-heading_&]:m-0 [.skill-scope-head_&]:mb-[2px] text-[13px] leading-5 font-bold tracking-[0] text-content-soft"
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
        'status-badge [.cli-settings-heading_&]:[align-self:center] h-auto min-h-6 rounded-full border-0 px-2 text-[11px] leading-4 font-bold tracking-[0]',
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
      className={cn(
        'app-switch data-[size=default]:h-8 data-[size=default]:w-12 data-checked:bg-primary data-unchecked:bg-control-muted [&_[data-slot=switch-thumb]]:size-6',
        density === 'compact' &&
          'data-[size=default]:h-6 data-[size=default]:w-10 [&_[data-slot=switch-thumb]]:size-5',
        className,
      )}
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
  return (
    <Tabs value={value} onValueChange={onChange} className={cn('block min-w-0', className)}>
      <TabsList
        className={cn(
          'segmented min-h-9 max-w-full justify-start gap-0.5 overflow-x-auto rounded-lg bg-surface-muted p-1 [scrollbar-width:none]',
          compact && 'compact w-auto border border-border bg-transparent',
        )}
      >
        {options.map((option) => (
          <TabsTrigger
            className="h-8 min-w-14 flex-none rounded-md border-0 px-3 text-[12px] font-semibold tracking-[0] text-content-muted data-active:bg-card data-active:text-content data-active:shadow-sm"
            value={option}
            key={option}
          >
            {option}
          </TabsTrigger>
        ))}
      </TabsList>
    </Tabs>
  )
}
