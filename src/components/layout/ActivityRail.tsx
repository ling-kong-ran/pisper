import type { ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'

export type ActivityGroup = {
  id: string
  label: string
  icon: LucideIcon
  items: Array<{ id: string; label: string; icon: LucideIcon; disabled?: boolean }>
}

export function ActivityRail({
  groups,
  activeId,
  onSelect,
  footer,
  label,
}: {
  groups: ActivityGroup[]
  activeId: string
  onSelect: (id: string) => void
  footer?: ReactNode
  label: string
}) {
  return (
    <nav
      aria-label={label}
      data-testid="activity-rail"
      className="relative z-20 flex h-full w-(--activity-rail-width) shrink-0 flex-col items-center bg-[var(--app-chrome)] py-2"
    >
      <div className="flex min-h-0 w-full flex-1 flex-col items-center gap-2 overflow-y-auto">
        {groups.map((group) => {
          const active = group.items.some((item) => item.id === activeId)
          return (
            <DropdownMenu key={group.id} modal={false}>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  className={cn(
                    'relative size-11 shrink-0 rounded-xl text-muted-foreground',
                    active && 'bg-surface-hover text-content',
                  )}
                  aria-label={group.label}
                  title={group.label}
                >
                  <group.icon className="size-5" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                side="right"
                align="start"
                sideOffset={8}
                className="w-56 max-w-[calc(100vw-60px)]"
              >
                <DropdownMenuLabel>{group.label}</DropdownMenuLabel>
                {group.items.map((item) => (
                  <DropdownMenuItem
                    key={item.id}
                    disabled={item.disabled}
                    onSelect={() => onSelect(item.id)}
                    aria-current={item.id === activeId ? 'page' : undefined}
                  >
                    <item.icon />
                    <span className="min-w-0 truncate">{item.label}</span>
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          )
        })}
      </div>
      <div className="flex w-full shrink-0 flex-col items-center gap-1 pt-2">{footer}</div>
    </nav>
  )
}
