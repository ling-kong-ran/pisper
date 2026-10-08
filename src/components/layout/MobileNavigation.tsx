// 设置页横滑条。手机主入口已改成左中右三屏，不再使用底部主导航。
import { useEffect, useMemo, useRef } from 'react'
import {
  getSettingsNavigation,
  settingsNavigationKey,
  type SettingsDestination,
} from '@/app/routes/settings-navigation'
import { useI18n } from '@/app/i18n/use-i18n'
import { cn } from '@/lib/utils'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

type MobileSettingsNavigationProps = {
  page: string
  configSection: string
  mobileApp: boolean
  onNavigate: (destination: SettingsDestination) => void
}

export function MobileSettingsNavigation({
  page,
  configSection,
  mobileApp,
  onNavigate,
}: MobileSettingsNavigationProps) {
  const { t } = useI18n()
  const activeKey = settingsNavigationKey(page, configSection)
  const activeItemRef = useRef<HTMLButtonElement | null>(null)
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const items = useMemo(
    () =>
      getSettingsNavigation(t, { mobileApp, capabilities })
        .flatMap((group) => group.items)
        .filter(
          (item) => item.destination.type !== 'config' || item.destination.id !== 'desktop-pet',
        ),
    [capabilities, mobileApp, t],
  )

  useEffect(() => {
    // 横滑导航可能比屏幕宽；切页后把当前项送回可视区，避免用户再寻找高亮项。
    activeItemRef.current?.scrollIntoView({
      behavior: 'smooth',
      block: 'nearest',
      inline: 'center',
    })
  }, [activeKey])

  return (
    <nav
      aria-label={t('config:settingsShell.settingsNavigation')}
      className="flex flex-none gap-2 overflow-x-auto px-4 pb-3 pt-1 [-ms-overflow-style:none] [scrollbar-width:none] max-[650px]:px-2 [&::-webkit-scrollbar]:hidden"
      data-mobile-navigation="settings"
    >
      {items.map((item) => {
        const active = activeKey === item.key
        const Icon = item.icon
        return (
          <button
            aria-current={active ? 'page' : undefined}
            className={cn(
              'flex flex-none items-center gap-1.5 rounded-full border px-3 py-1.5 text-[12px] font-medium transition-colors',
              active
                ? 'border-[var(--star-strong)] bg-[var(--star-soft)] text-[var(--star-strong)]'
                : 'border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text)]',
            )}
            key={item.key}
            onClick={() => onNavigate(item.destination)}
            ref={active ? activeItemRef : undefined}
          >
            <Icon aria-hidden="true" size={14} />
            <span>{item.label}</span>
          </button>
        )
      })}
    </nav>
  )
}
