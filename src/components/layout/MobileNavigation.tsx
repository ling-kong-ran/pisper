// 移动端 App 导航：底部栏承载五个主入口，设置横滑条覆盖全部设置页。
// 抽屉仍保留最近会话和完整导航，避免牺牲深层入口与会话切换能力。
import { useEffect, useMemo, useRef } from 'react'
import { Settings } from 'lucide-react'
import { getNavigation } from '@/app/navigation'
import {
  getSettingsNavigation,
  SETTINGS_PAGES,
  settingsNavigationKey,
  type SettingsDestination,
} from '@/app/settings-navigation'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

type MobilePrimaryNavigationProps = {
  page: string
  onNavigate: (page: string) => void
  composerFocused: boolean
}

export function MobilePrimaryNavigation({
  page,
  onNavigate,
  composerFocused,
}: MobilePrimaryNavigationProps) {
  const { t } = useI18n()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const items = useMemo(
    () => getNavigation(t, capabilities).flatMap(([, groupItems]) => groupItems),
    [capabilities, t],
  )
  const activePage =
    page === 'chatHistory' ? 'chat' : page === 'workflowCreate' ? 'workflows' : page

  return (
    <nav
      aria-label={t('navigation:appSidebar.mainNavigation')}
      className="[[data-mobile-keyboard='open']_&]:hidden flex h-[calc(58px_+_env(safe-area-inset-bottom))] flex-none items-stretch border-t border-[var(--stroke)] bg-[var(--sidebar-bg)] pb-[env(safe-area-inset-bottom)] shadow-[0_-8px_24px_-20px_var(--main-surface-shadow)]"
      data-mobile-navigation="primary"
      hidden={composerFocused}
    >
      {items.map(([id, label, Icon]) => {
        const active = activePage === id
        return (
          <button
            aria-current={active ? 'page' : undefined}
            className={cn(
              'relative flex min-w-0 flex-1 flex-col items-center justify-center gap-0.5 text-[10px] font-medium transition-colors',
              active ? 'text-[var(--star-strong)]' : 'text-[var(--text-muted)]',
            )}
            key={id}
            onClick={() => onNavigate(id)}
          >
            <span
              aria-hidden="true"
              className={cn(
                'absolute inset-x-[30%] top-0 h-0.5 rounded-full',
                active ? 'bg-[var(--star-strong)]' : 'bg-transparent',
              )}
            />
            <Icon aria-hidden="true" size={19} strokeWidth={active ? 2.2 : 1.8} />
            <span className="max-w-full whitespace-nowrap">{label}</span>
          </button>
        )
      })}
      <button
        aria-current={SETTINGS_PAGES.has(page) ? 'page' : undefined}
        className={cn(
          'relative flex min-w-0 flex-1 flex-col items-center justify-center gap-0.5 text-[10px] font-medium transition-colors',
          SETTINGS_PAGES.has(page) ? 'text-[var(--star-strong)]' : 'text-[var(--text-muted)]',
        )}
        onClick={() => onNavigate('config')}
      >
        <span
          aria-hidden="true"
          className={cn(
            'absolute inset-x-[30%] top-0 h-0.5 rounded-full',
            SETTINGS_PAGES.has(page) ? 'bg-[var(--star-strong)]' : 'bg-transparent',
          )}
        />
        <Settings aria-hidden="true" size={19} strokeWidth={SETTINGS_PAGES.has(page) ? 2.2 : 1.8} />
        <span className="max-w-full whitespace-nowrap">{t('navigation:navigation.settings')}</span>
      </button>
    </nav>
  )
}

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
