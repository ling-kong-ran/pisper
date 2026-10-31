// 设置整页的侧栏（规格 v2 第 2.3、2.6 节）：进入设置时整块侧栏换成“← 返回 Esc”+ 设置目录
// （模型 · 界面 · 通知 · 快捷键 · 更新 · 关于）+ 设置搜索，目录最下面一行“插件、MCP、技能等在 功能 里 →”；
// 底部常驻一行“● 3 个需要你 · ◠ 3 个运行中 ›”（插槽，看板数据层懒加载），点开回到全部代理。
// 主区只有设置内容，不再叠第三列分区导航（取代 SettingsSectionNav）。
// Esc（焦点不在输入框、没有打开的弹层，也没有别的返回层接住）与“← 返回”回到进入设置前的页面；
// 这是“Esc 永远不用于返回”的唯一例外，侧栏上写明了 Esc（规格 2.3）。焦点回到原会话行由 SidebarCommands 负责。
// 本模块只在设置页按需加载，不进入口包。
import type { ReactNode } from 'react'
import { ArrowLeft, ArrowRight } from 'lucide-react'
import { getSettingsNavigation } from '@/app/settings-navigation'
import { useI18n } from '@/app/use-i18n'
import { Kbd } from '@/components/ui/kbd'
import { useEscapeLeave } from '@/hooks/use-escape-leave'
import { cn } from '@/lib/utils'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

export type SettingsSidebarProps = {
  configSection: string
  onSelectSection: (section: string) => void
  onBack: () => void
  onOpenFeatures: () => void
  // 设置搜索框（沿用 ConfigSearch），由外壳懒加载后传入。
  search?: ReactNode
  // “关于”行尾的版本号。
  version?: string
  // 底部常驻的并行状态行。
  footer?: ReactNode
}

// 目录行 32px、16px 图标，文字跟随界面字号（沿用原设置分区导航的钉子：行高与图标不随字号变）。
const ROW =
  'flex h-8 w-full min-w-0 items-center gap-2.5 rounded-[var(--r-md)] px-2.5 text-left text-[length:var(--app-font-size)] text-content transition-colors duration-(--d-fast) hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11'

export function SettingsSidebar({
  configSection,
  onSelectSection,
  onBack,
  onOpenFeatures,
  search,
  version,
  footer,
}: SettingsSidebarProps) {
  const { t } = useI18n()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const items = getSettingsNavigation(t, { capabilities })
  // Esc 返回（焦点不在输入框、没有弹层，也没有别的返回层接住时），规则见 useEscapeLeave。
  useEscapeLeave(onBack)

  const backLabel = t('config:settingsShell.back')
  return (
    <nav
      aria-label={t('config:settingsShell.settingsNavigation')}
      data-settings-sidebar
      className="flex min-h-0 flex-1 flex-col animate-in fade-in duration-(--d-fast) motion-reduce:animate-none"
    >
      <div
        data-window-drag-region
        className="flex h-[52px] flex-none items-center gap-2 pr-3.5 pl-2"
      >
        <button
          type="button"
          data-settings-back
          aria-keyshortcuts="Escape"
          className="flex h-8 items-center gap-2 rounded-[var(--r-md)] pr-2.5 pl-1.5 text-sm text-content-secondary transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11"
          onClick={onBack}
        >
          <ArrowLeft className="size-[18px]" strokeWidth={1.75} aria-hidden="true" />
          {backLabel}
        </button>
        <span className="flex-1" />
        <Kbd
          aria-hidden="true"
          className="border-transparent bg-transparent text-content-faint shadow-none pointer-coarse:hidden"
        >
          Esc
        </Kbd>
      </div>
      <div className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto overscroll-contain px-2 pb-2">
        <h2 className="m-0 px-2.5 pt-2.5 pb-3 text-xl leading-7 font-semibold tracking-[-.01em] text-content">
          {t('navigation:navigation.settings')}
        </h2>
        {search && <div className="relative z-20 mb-2 px-0.5">{search}</div>}
        <ul className="m-0 flex list-none flex-col gap-px p-0">
          {items.map((item) => {
            const active = item.section === configSection
            return (
              <li key={item.section}>
                <button
                  type="button"
                  data-settings-section={item.section}
                  aria-current={active ? 'page' : undefined}
                  className={cn(
                    ROW,
                    active && 'bg-surface-active font-medium hover:bg-surface-active',
                  )}
                  onClick={() => onSelectSection(item.section)}
                >
                  <item.icon
                    className="size-4 flex-none text-content-secondary"
                    strokeWidth={1.75}
                    aria-hidden="true"
                  />
                  <span className="min-w-0 flex-1 truncate">{item.label}</span>
                  {item.section === 'about' && version && (
                    <small className="font-mono text-xs font-normal text-content-muted tabular-nums">
                      v{version}
                    </small>
                  )}
                </button>
              </li>
            )
          })}
        </ul>
        <button
          type="button"
          data-settings-features-link
          className="mt-4 flex min-h-8 w-full items-center gap-1 rounded-[var(--r-md)] px-2.5 py-1 text-left text-xs leading-5 text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:min-h-11"
          onClick={onOpenFeatures}
        >
          <span className="min-w-0 flex-1">{t('config:settingsShell.featuresHint')}</span>
          <ArrowRight className="size-3.5 flex-none" aria-hidden="true" />
        </button>
      </div>
      {footer && <div className="flex-none p-2">{footer}</div>}
    </nav>
  )
}
