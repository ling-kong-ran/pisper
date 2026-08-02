import type { ComponentProps, ReactNode } from 'react'
import { Badge } from '@/components/ui/badge'
import { Card, CardTitle } from '@/components/ui/card'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'

const BADGE_TONES = {
  blue: 'bg-[var(--brand-blue-soft)] text-[var(--brand-blue-strong)]',
  green: 'bg-[var(--success-soft)] text-[var(--success-strong)]',
  red: 'bg-[var(--danger-soft)] text-[var(--danger)]',
  amber: 'bg-[var(--warning-soft)] text-[var(--warning-strong)]',
  gray: 'bg-[var(--surface-muted)] text-[var(--text-muted)]',
} as const

export type SettingsBadgeTone = keyof typeof BADGE_TONES

export function SettingsCard({ className, ...props }: ComponentProps<typeof Card>) {
  return (
    <Card
      className={cn(
        'panel block min-w-0 gap-0 overflow-visible rounded-[10px] border border-[var(--stroke)] bg-[var(--panel)] p-3.5 text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] ring-0 backdrop-blur-[14px]',
        className,
      )}
      {...props}
    />
  )
}

export function SettingsSectionTitle({ title }: { title: ReactNode }) {
  return (
    <CardTitle
      role="heading"
      aria-level={3}
      className="section-title text-[13px] font-bold text-[var(--text-soft)]"
    >
      {title}
    </CardTitle>
  )
}

export function SettingsBadge({
  children,
  tone = 'blue',
  className,
  ...props
}: Omit<ComponentProps<typeof Badge>, 'variant'> & { tone?: SettingsBadgeTone }) {
  return (
    <Badge
      variant="secondary"
      className={cn(
        'badge h-auto min-h-6 rounded-full border-0 px-2 text-[11px] leading-4 font-bold',
        BADGE_TONES[tone],
        className,
      )}
      {...props}
    >
      {children}
    </Badge>
  )
}

export function SettingsSwitch({
  value,
  onChange,
  ariaLabel,
  className,
  ...props
}: Omit<
  ComponentProps<typeof Switch>,
  'aria-label' | 'checked' | 'onChange' | 'onCheckedChange' | 'value'
> & {
  value: boolean
  onChange: (value: boolean) => void
  ariaLabel?: string
}) {
  return (
    <Switch
      checked={value}
      onCheckedChange={onChange}
      aria-label={ariaLabel}
      className={cn(
        'toggle data-[size=default]:h-6 data-[size=default]:w-10 data-checked:bg-[var(--star)] data-unchecked:bg-[var(--control-muted)] [&_[data-slot=switch-thumb]]:size-5',
        className,
      )}
      onClick={(event) => event.stopPropagation()}
      {...props}
    />
  )
}
