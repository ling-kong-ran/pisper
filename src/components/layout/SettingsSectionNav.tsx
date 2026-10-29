// 设置页的页内分区导航（桌面）：复用 getSettingsNavigation 的分组与 settingsNavigationKey 高亮，
// 只在设置拥有的页面里、内容区左侧出现，不再替换应用级侧栏。手机仍用 MobileSettingsNavigation 横滑条。
import { useMemo, type ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import {
  getSettingsNavigation,
  settingsNavigationKey,
  type SettingsDestination,
} from '@/app/settings-navigation'
import { cn } from '@/lib/utils'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

type SettingsSectionNavProps = {
  page: string
  configSection: string
  mobileApp: boolean
  onNavigate: (destination: SettingsDestination) => void
  // 设置搜索框（配置分区内搜索），由外壳按需懒加载后传入。
  search?: ReactNode
  // 「关于」行尾的版本号。
  version?: string
}

export function SettingsSectionNav({
  page,
  configSection,
  mobileApp,
  onNavigate,
  search,
  version,
}: SettingsSectionNavProps) {
  const { t } = useI18n()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const groups = useMemo(
    () => getSettingsNavigation(t, { mobileApp, capabilities }),
    [capabilities, mobileApp, t],
  )
  const activeKey = settingsNavigationKey(page, configSection)
  // 行高 32px、16px 图标、正文字号跟随界面字号；选中行抬起为键帽并带 3px 强调色刻度，
  // 不只依赖底色差异表达选中。
  const item =
    'relative flex h-8 w-full min-w-0 items-center gap-2.5 rounded-sm px-2.5 text-left text-[length:var(--app-font-size)] text-content-secondary transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus in-data-[density=compact]:h-[30px] pointer-coarse:h-11 [&_svg]:text-content-muted'
  const itemActive =
    'bg-key font-medium text-content shadow-key ring-1 ring-key-border hover:bg-key [&_svg]:text-accent-fg before:absolute before:top-1/2 before:left-0 before:h-4 before:w-[3px] before:-translate-y-1/2 before:rounded-r-[3px] before:bg-star'
  return (
    <aside className="flex w-[232px] flex-none flex-col border-r border-stroke-soft bg-panel">
      {search && <div className="relative z-20 flex-none px-2 pt-3 pb-1">{search}</div>}
      <nav
        aria-label={t('config:settingsShell.settingsNavigation')}
        className="min-h-0 flex-1 overflow-y-auto px-2 pt-1 pb-3"
      >
        {groups.map((group) => (
          <div key={group.label} className="mt-3 flex flex-col gap-0.5 first:mt-1">
            <span className="flex h-7 items-center px-2.5 text-xs font-medium tracking-[.02em] text-content-muted">
              {group.label}
            </span>
            {group.items.map((entry) => {
              const active = activeKey === entry.key
              return (
                <button
                  key={entry.key}
                  type="button"
                  className={cn(item, active && itemActive)}
                  aria-current={active ? 'page' : undefined}
                  onClick={() => onNavigate(entry.destination)}
                >
                  <entry.icon size={16} aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate">{entry.label}</span>
                  {entry.key === 'config:about' && version && (
                    <small className="ml-auto font-mono text-xs font-normal text-content-muted tabular-nums">
                      v{version}
                    </small>
                  )}
                </button>
              )
            })}
          </div>
        ))}
      </nav>
    </aside>
  )
}
