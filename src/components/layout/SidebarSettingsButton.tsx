import { Settings } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

export function SidebarSettingsButton({
  onOpen,
  active = false,
  compact = false,
}: {
  onOpen: () => void
  active?: boolean
  compact?: boolean
}) {
  const { t } = useI18n()
  return (
    <Button
      variant="ghost"
      onClick={onOpen}
      aria-current={active ? 'true' : undefined}
      title={compact ? t('navigation:navigation.settings') : undefined}
      className={cn(
        'h-11 w-full justify-start gap-2.5 rounded-lg px-2.5 text-sm font-normal text-foreground transition-colors hover:bg-sidebar-accent',
        active && 'bg-sidebar-accent',
        compact && 'justify-center px-0',
      )}
    >
      <Settings className="size-4" aria-hidden="true" />
      <span className={compact ? 'sr-only' : undefined}>{t('navigation:navigation.settings')}</span>
    </Button>
  )
}
