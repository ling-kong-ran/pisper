import type { ComponentProps, ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { BrandLogo } from '@/components/BrandLogo'
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

// 功能栏条目：宽屏（≥1024px，栏宽 64px）显示 18px 图标 + 12px 文字的 50×52 键位；
// 更窄时退回 44×44 纯图标，保证触控命中。选中项抬起为键帽，并在窗口边缘画 3px 强调色刻度，
// 刻度用「(条目宽 − 栏宽) / 2」贴齐栏的左缘，不依赖具体栏宽。
export function ActivityRailButton({
  icon: Icon,
  label,
  shortLabel,
  active = false,
  className,
  ...props
}: ComponentProps<'button'> & {
  icon: LucideIcon
  label: string
  shortLabel?: string
  active?: boolean
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      data-active={active || undefined}
      className={cn(
        'relative flex size-11 shrink-0 flex-col items-center justify-center gap-1 rounded-md text-content-muted transition-colors duration-[var(--d-fast)] hover:bg-surface-hover hover:text-content min-[1024px]:h-[52px] min-[1024px]:w-[50px]',
        active &&
          'bg-key text-content shadow-key ring-1 ring-key-border hover:bg-key before:absolute before:top-1/2 before:left-[calc((100%-var(--activity-rail-width))/2)] before:h-5 before:w-[3px] before:-translate-y-1/2 before:rounded-r-[3px] before:bg-star',
        className,
      )}
      {...props}
    >
      <Icon className="size-[18px]" strokeWidth={active ? 2 : 1.75} aria-hidden="true" />
      <span
        aria-hidden="true"
        className="hidden max-w-full truncate px-0.5 text-xs leading-none tracking-[.02em] min-[1024px]:block"
      >
        {shortLabel || label}
      </span>
    </button>
  )
}

export function ActivityRail({
  groups,
  activeId,
  onSelect,
  footer,
  label,
  shortLabels = {},
}: {
  groups: ActivityGroup[]
  activeId: string
  onSelect: (id: string) => void
  footer?: ReactNode
  label: string
  // 功能栏可见文字只有 12px 宽度预算，长分组名（如“工作台与项目”）按组 id 换成短名。
  shortLabels?: Record<string, string>
}) {
  return (
    <nav
      aria-label={label}
      data-testid="activity-rail"
      className="relative z-20 flex h-full w-(--activity-rail-width) shrink-0 flex-col items-center bg-chrome py-3"
    >
      {/* 品牌键帽只做识别，不可交互；在 Windows 自绘标题栏下顺带作为窗口拖动区域。 */}
      <div
        className="mb-3 grid w-full shrink-0 place-items-center min-[1024px]:mb-4"
        data-window-drag-region
      >
        <BrandLogo size={34} className="max-[650px]:size-[30px]" />
      </div>
      <div className="flex min-h-0 w-full flex-1 flex-col items-center gap-1 overflow-y-auto">
        {groups.map((group) => {
          const active = group.items.some((item) => item.id === activeId)
          return (
            <DropdownMenu key={group.id} modal={false}>
              <DropdownMenuTrigger asChild>
                <ActivityRailButton
                  icon={group.icon}
                  label={group.label}
                  shortLabel={shortLabels[group.id]}
                  active={active}
                />
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
