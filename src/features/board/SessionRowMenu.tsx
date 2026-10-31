// 会话行的 ⋯ 菜单（侧栏行与全部代理行共用，规格 2.2、3.3）：打开、置顶 / 取消置顶、重命名、
// 标记已处理（仅等你处理的失败）、复制会话链接、归档。动作实现在 useSessionRowActions。
import { MoreHorizontal } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import type { SessionRowActions, SessionRowTarget } from './useSessionRowActions'

export function SessionRowMenu({
  row,
  actions,
  canMarkHandled,
  open,
  onOpenChange,
  side,
  align = 'start',
  tabIndex,
  className,
}: {
  row: SessionRowTarget
  actions: SessionRowActions
  // 失败还在“需要你”时才有“标记已处理”。
  canMarkHandled: boolean
  open?: boolean
  onOpenChange?: (open: boolean) => void
  side?: 'right' | 'bottom'
  align?: 'start' | 'end'
  // 全部代理行用 roving tabindex：行不是当前行时 ⋯ 也退出 Tab 序列。
  tabIndex?: number
  className?: string
}) {
  const { t } = useI18n()
  return (
    <DropdownMenu open={open} onOpenChange={onOpenChange}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          tabIndex={tabIndex}
          aria-label={t('board:sidebar.rowMenu', { title: row.title })}
          className={cn(
            'grid size-[26px] flex-none place-items-center rounded-[var(--r-sm)] text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-active hover:text-content focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus',
            className,
          )}
        >
          <MoreHorizontal className="size-4" aria-hidden="true" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align={align} side={side} className="min-w-44">
        <DropdownMenuItem onSelect={() => actions.open(row)}>
          {t('board:menu.open')}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => actions.pin(row)}>
          {row.pinned ? t('board:menu.unpin') : t('board:menu.pin')}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => actions.rename(row)}>
          {t('board:menu.rename')}
        </DropdownMenuItem>
        {canMarkHandled && (
          <DropdownMenuItem onSelect={() => actions.markHandled(row)}>
            {t('board:menu.markHandled')}
          </DropdownMenuItem>
        )}
        <DropdownMenuItem onSelect={() => actions.copyLink(row)}>
          {t('board:menu.copyLink')}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => actions.archive(row)}>
          {t('board:menu.archive')}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
