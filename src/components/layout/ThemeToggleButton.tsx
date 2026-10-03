import { Clock, MonitorCog, Moon, Sun, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { nextThemeMode, type ThemeMode } from '@/stores/ui-store'

const THEME_ICONS: Record<ThemeMode, LucideIcon> = {
  system: MonitorCog,
  scheduled: Clock,
  dark: Moon,
  light: Sun,
}

// 会话与其他页头使用同一份模式提示；跟随系统与显式主题同色时也能辨别状态。
export function ThemeToggleButton({
  theme,
  onCycle,
  compact = false,
  className,
}: {
  theme: ThemeMode
  onCycle: () => void
  compact?: boolean
  className?: string
}) {
  const { t } = useI18n()
  const labels: Record<ThemeMode, string> = {
    system: t('navigation:pageHeader.system'),
    scheduled: t('navigation:pageHeader.scheduled'),
    dark: t('navigation:pageHeader.dark'),
    light: t('navigation:pageHeader.light'),
  }
  const Icon = THEME_ICONS[theme]
  const description = { theme: labels[theme], next: labels[nextThemeMode(theme)] }
  return (
    <Button
      variant={compact ? 'ghost' : 'outline'}
      size="icon"
      className={cn(
        '[-webkit-app-region:no-drag]',
        compact && 'size-8 rounded-lg text-muted-foreground hover:bg-muted hover:text-foreground',
        className,
      )}
      data-testid="theme-toggle"
      title={t('navigation:pageHeader.themeThemeClickToSwitch', description)}
      aria-label={t('navigation:pageHeader.themeThemeClickToSwitchThemes', description)}
      onClick={onCycle}
    >
      <Icon className="size-4" aria-hidden="true" />
    </Button>
  )
}
