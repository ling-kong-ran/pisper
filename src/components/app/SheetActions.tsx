// 手机底部表单里的两种操作：顶部的快捷块（最常用的 2–3 项）和下面的行。
// “+”表单与会话菜单共用，尺寸和图标颜色只在这里定一次；行高 44、块高 68，都满足触控目标。
import type { ComponentProps, ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { cn } from '@/lib/utils'

const FOCUS_RING =
  'outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus'

const TILE_COLUMNS: Record<number, string> = {
  1: 'grid-cols-1',
  2: 'grid-cols-2',
  3: 'grid-cols-3',
}

export function SheetTileGrid({ count, children }: { count: number; children: ReactNode }) {
  return (
    <div className={cn('grid gap-2 px-1 pb-2', TILE_COLUMNS[count] ?? 'grid-cols-3')}>
      {children}
    </div>
  )
}

export function SheetTile({
  icon: Icon,
  label,
  className,
  ...props
}: Omit<ComponentProps<'button'>, 'children'> & { icon: LucideIcon; label: string }) {
  return (
    <button
      type="button"
      className={cn(
        'flex h-[68px] min-w-0 cursor-pointer flex-col items-center justify-center gap-1.5 rounded-[var(--r-md)] bg-surface-muted px-1 text-[13px] text-content active:bg-surface-hover disabled:cursor-default disabled:text-content-faint',
        FOCUS_RING,
        className,
      )}
      {...props}
    >
      <Icon className="size-5 text-content-secondary" strokeWidth={1.75} aria-hidden="true" />
      <span className="max-w-full truncate">{label}</span>
    </button>
  )
}

export function SheetRow({
  icon: Icon,
  label,
  hint,
  trailing,
  className,
  ...props
}: Omit<ComponentProps<'button'>, 'children'> & {
  icon: LucideIcon
  label: string
  hint?: string
  trailing?: ReactNode
}) {
  return (
    <button
      type="button"
      className={cn(
        'flex min-h-11 w-full min-w-0 cursor-pointer items-center gap-3 rounded-[var(--r-md)] px-3 text-left text-[15px] text-content active:bg-surface-hover disabled:cursor-default disabled:text-content-faint',
        FOCUS_RING,
        className,
      )}
      {...props}
    >
      <Icon
        className="size-[18px] flex-none text-content-secondary"
        strokeWidth={1.75}
        aria-hidden="true"
      />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate">{label}</span>
        {hint && <span className="truncate text-xs text-content-muted">{hint}</span>}
      </span>
      {trailing}
    </button>
  )
}
