// 移动端设置横滑条；主导航由常驻功能栏统一提供。
import { useEffect, useMemo, useRef } from 'react'
import {
  getSettingsNavigation,
  settingsNavigationKey,
  type SettingsDestination,
} from '@/app/settings-navigation'
import { useI18n } from '@/app/use-i18n'
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
      className="flex flex-none gap-2 overflow-x-auto border-b border-stroke-soft px-4 pt-1 pb-3 [-ms-overflow-style:none] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      data-mobile-navigation="settings"
    >
      {items.map((item) => {
        const active = activeKey === item.key
        const Icon = item.icon
        return (
          <button
            aria-current={active ? 'page' : undefined}
            // 横滑分区是 44px 触控标签：选中用强调色浅底 + 强调色描边与文字，不靠细微底色差。
            className={cn(
              'flex h-11 flex-none items-center gap-1.5 rounded-md border px-3 text-[13px] transition-colors duration-[var(--d-fast)]',
              active
                ? 'border-star-border bg-star-soft font-medium text-star-strong'
                : 'border-stroke text-content-secondary active:bg-surface-subtle',
            )}
            key={item.key}
            onClick={() => onNavigate(item.destination)}
            ref={active ? activeItemRef : undefined}
          >
            <Icon aria-hidden="true" size={16} />
            <span>{item.label}</span>
          </button>
        )
      })}
    </nav>
  )
}
