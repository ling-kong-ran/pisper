// 账户区只编排菜单；业务导航由应用壳传入，整行共用一个键盘焦点。
import { ChevronUp, Palette, Server } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'

export default function SidebarAccountMenu({
  onProvider,
  onAppearance,
}: {
  onProvider: () => void
  onAppearance: () => void
}) {
  const { t } = useI18n()
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={t('navigation:appSidebar.accountMenu')}
          className="flex h-11 w-full items-center gap-2.5 rounded-lg px-1.5 text-left hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-ring"
        >
          <span
            aria-hidden="true"
            className="grid size-8 shrink-0 place-items-center rounded-full bg-foreground text-sm font-medium text-background"
          >
            P
          </span>
          <span className="min-w-0 flex-1 truncate text-sm font-medium">Pisper</span>
          <ChevronUp size={15} className="text-muted-foreground" aria-hidden="true" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="start" sideOffset={8} className="w-56">
        <DropdownMenuItem onSelect={onProvider}>
          <Server size={16} />
          {t('navigation:appSidebar.providerSettings')}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={onAppearance}>
          <Palette size={16} />
          {t('navigation:appSidebar.appearanceSettings')}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
