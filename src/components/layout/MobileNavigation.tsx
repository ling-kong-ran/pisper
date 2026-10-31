// 手机设置（规格 v2 第 7.4、2.6 节）：列表 → 分区。/config 是分区列表（与桌面设置目录同一份
// getSettingsNavigation，只有应用设置），点一行进入 /config/<分区>；分区页顶上是“‹ 设置”回到列表
// （iOS 没有返回键，Android 返回键同样回列表，见 shell-tabs.shellBackPath）。
// 列表最后一行“插件、MCP、技能等在 功能 里 →”：这些页只在功能里（双归属已退役）。
// 第 4 期的横滑分区条随之退役。
import { useMemo } from 'react'
import { ChevronLeft, ChevronRight } from 'lucide-react'
import { getSettingsNavigation } from '@/app/settings-navigation'
import { useI18n } from '@/app/use-i18n'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

const ROW =
  'flex min-h-14 w-full min-w-0 items-center gap-3 px-4 text-left text-base text-content focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus active:bg-surface-hover'

export function MobileSettingsList({
  onNavigate,
  onOpenFeatures,
}: {
  onNavigate: (section: string) => void
  onOpenFeatures: () => void
}) {
  const { t } = useI18n()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const items = useMemo(() => getSettingsNavigation(t, { capabilities }), [capabilities, t])
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto bg-background">
      {/* 页名在外壳顶栏里，这里的 h1 只给读屏。 */}
      <h1 className="sr-only">{t('navigation:navigation.settings')}</h1>
      <nav
        aria-label={t('config:settingsShell.settingsNavigation')}
        data-mobile-navigation="settings"
        className="mx-4 mt-2 overflow-hidden rounded-[14px] border border-stroke-soft bg-panel"
      >
        <ul className="m-0 list-none p-0 [&>li+li]:border-t [&>li+li]:border-stroke-soft">
          {items.map((item) => {
            const Icon = item.icon
            return (
              <li key={item.section}>
                <button
                  type="button"
                  data-settings-section={item.section}
                  className={ROW}
                  onClick={() => onNavigate(item.section)}
                >
                  <Icon
                    className="size-[18px] flex-none text-content-secondary"
                    strokeWidth={1.75}
                    aria-hidden="true"
                  />
                  <span className="min-w-0 flex-1 truncate">{item.label}</span>
                  <ChevronRight
                    className="size-4 flex-none text-content-faint"
                    aria-hidden="true"
                  />
                </button>
              </li>
            )
          })}
        </ul>
      </nav>
      <button
        type="button"
        data-settings-features-hint
        className="mx-4 mt-3 flex min-h-11 items-center rounded-[var(--r-md)] px-1 text-left text-sm text-content-muted focus-visible:outline-2 focus-visible:outline-focus active:text-content"
        onClick={onOpenFeatures}
      >
        {t('config:settingsShell.featuresHint')}
      </button>
    </div>
  )
}

export function MobileSettingsBack({ onBack }: { onBack: () => void }) {
  const { t } = useI18n()
  return (
    <div className="flex flex-none px-2 pt-1">
      <button
        type="button"
        data-settings-back
        className="flex h-11 items-center gap-0.5 rounded-[var(--r-md)] pr-3 pl-1 text-sm text-content-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus active:bg-surface-hover"
        onClick={onBack}
      >
        <ChevronLeft className="size-5" aria-hidden="true" />
        {t('navigation:navigation.settings')}
      </button>
    </div>
  )
}
