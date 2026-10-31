// 移动端设置横滑条：与桌面设置侧栏同一份设置目录（getSettingsNavigation，规格 2.6 只有应用设置）；
// 插件、远程访问 / 手机服务器、桌面宠物等功能页只在“更多”（功能）里。列表 → 分区的手机设置在 A4。
import { useEffect, useMemo, useRef } from 'react'
import { getSettingsNavigation } from '@/app/settings-navigation'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

type MobileSettingsNavigationProps = {
  configSection: string
  onNavigate: (section: string) => void
}

export function MobileSettingsNavigation({
  configSection,
  onNavigate,
}: MobileSettingsNavigationProps) {
  const { t } = useI18n()
  const activeKey = configSection
  const activeItemRef = useRef<HTMLButtonElement | null>(null)
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const items = useMemo(() => getSettingsNavigation(t, { capabilities }), [capabilities, t])

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
        const active = activeKey === item.section
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
            key={item.section}
            onClick={() => onNavigate(item.section)}
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
