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
        'app-card block min-w-0 gap-0 overflow-visible rounded-surface border border-border bg-card p-3.5 py-3.5 text-card-foreground shadow-surface ring-0 backdrop-blur-[14px]',
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
      className="app-section-title text-[13px] leading-5 font-bold tracking-[0] text-content-soft"
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
        'status-badge h-auto min-h-6 rounded-full border-0 px-2 text-[11px] leading-4 font-bold tracking-[0]',
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
