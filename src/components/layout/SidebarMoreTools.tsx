// Lazy utility navigation keeps menu primitives out of the startup dependency graph.
import { Ellipsis, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from '@/components/ui/dropdown-menu'

export default function SidebarMoreTools({
  items,
  buttonClassName,
  onNavigate,
}: {
  items: Array<[string, string, LucideIcon]>
  buttonClassName: string
  onNavigate: (id: string) => void
}) {
  const { t } = useI18n()
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className={buttonClassName}>
          <Ellipsis size={16} />
          <span>{t('navigation:workbench.moreTools')}</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="right" align="start" className="w-48">
        {items.map(([id, label, Icon]) => (
          <DropdownMenuItem key={id} onSelect={() => onNavigate(id)}>
            <Icon size={16} />
            {label}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
