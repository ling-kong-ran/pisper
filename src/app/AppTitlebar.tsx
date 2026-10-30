// 48px 应用标题栏：品牌 · 主标签（会话/看板/组件/更多/设置）· ⌘K 搜索 · 舰队计量 · 新任务。
// 主页是会话：“会话”标签排第一，会话页与主页点亮它，点它回到上次停留的会话或新任务草稿。
// 桌面没有侧栏和活动栏，一级导航全在这一栏。Windows 自绘标题栏时整栏是拖拽区（交互元素除外，
// 双击最大化沿用 DesktopWindowControls 的机制），右侧 144px 让给窗口按钮；macOS/Linux 只留 12px。
// 1080 宽时的收缩顺序：搜索先缩成 32px 图标，舰队计量去掉文字只留图标 + 数字，“需要你”始终带字。
// 浏览器里比桌面最小窗口更窄（但仍不是手机）时再收起字标与标签文字，保证不出现横向溢出。
// 触屏（手机 App 平板、触屏电脑）上标题栏仍是 48px：标签与搜索键可见尺寸不变，命中区用伪元素补到 44px
//（TOUCH_HIT_AREA）；“新任务”与“需要你”本身就抬到 44px 高。
import type { ReactNode } from 'react'
import { Blocks, LayoutGrid, Plus, Search, Settings, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { ShellTab } from '@/app/shell-tabs'
import { BrandLogo } from '@/components/BrandLogo'
import { LaneBarsIcon, SessionBubbleIcon } from '@/components/app/LaneBarsIcon'
import { Button } from '@/components/ui/button'
import { Kbd } from '@/components/ui/kbd'
import { useShortcutLabel } from '@/lib/shortcuts'
import { TOUCH_HIT_AREA } from '@/lib/touch-target'
import { cn } from '@/lib/utils'

const TAB_ICONS: Record<ShellTab, LucideIcon> = {
  session: SessionBubbleIcon,
  board: LaneBarsIcon,
  components: Blocks,
  more: LayoutGrid,
  settings: Settings,
}

type AppTitlebarProps = {
  activeTab: ShellTab | null
  onSelectTab: (tab: ShellTab) => void
  onOpenPalette: () => void
  onNewTask: () => void
  // 舰队计量依赖看板数据层，按需加载后填入；加载前留同宽的安静占位。
  fleet: ReactNode
  // 有可用更新时的提示键（原侧栏底部的更新状态），没有更新时为空。
  status?: ReactNode
}

export function AppTitlebar({
  activeTab,
  onSelectTab,
  onOpenPalette,
  onNewTask,
  fleet,
  status,
}: AppTitlebarProps) {
  const { t } = useI18n()
  const paletteShortcut = useShortcutLabel('commandPalette')
  const newTaskShortcut = useShortcutLabel('primaryAction')
  const tabShortcuts: Record<ShellTab, string> = {
    session: '',
    board: useShortcutLabel('openBoard'),
    components: useShortcutLabel('openComponents'),
    more: useShortcutLabel('openMore'),
    settings: useShortcutLabel('openSettings'),
  }
  const tabs: Array<{ id: ShellTab; label: string }> = [
    { id: 'session', label: t('navigation:navigation.chat') },
    { id: 'board', label: t('navigation:shell.board') },
    { id: 'components', label: t('navigation:shell.components') },
    { id: 'more', label: t('navigation:shell.more') },
    { id: 'settings', label: t('navigation:navigation.settings') },
  ]
  return (
    <header
      data-window-drag-region
      data-app-titlebar
      className="relative z-30 flex h-12 flex-none items-center gap-1.5 border-b border-stroke bg-chrome pr-3 pl-3 [[data-custom-titlebar]_&]:pr-[144px]"
    >
      <div className="mr-1.5 flex h-6 flex-none items-center gap-2 border-r border-stroke pr-3.5 pl-1">
        <BrandLogo size={22} />
        <span
          aria-hidden="true"
          className="text-sm font-semibold tracking-[-.01em] text-content max-[959px]:hidden"
        >
          Pisper
        </span>
      </div>
      <nav
        aria-label={t('navigation:shell.primaryNavigation')}
        className="flex flex-none items-center gap-0.5"
      >
        {tabs.map((tab) => {
          const Icon = TAB_ICONS[tab.id]
          const active = activeTab === tab.id
          const shortcut = tabShortcuts[tab.id]
          return (
            <button
              key={tab.id}
              type="button"
              aria-current={active ? 'page' : undefined}
              title={shortcut ? `${tab.label} (${shortcut})` : tab.label}
              data-shell-tab={tab.id}
              className={cn(
                'flex h-8 items-center gap-1.5 rounded-sm px-2.5 text-sm font-medium whitespace-nowrap transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus',
                TOUCH_HIT_AREA,
                active
                  ? 'bg-solid text-content shadow-edge ring-1 ring-stroke-strong ring-inset'
                  : 'text-content-secondary hover:bg-surface-hover hover:text-content',
              )}
              onClick={() => onSelectTab(tab.id)}
            >
              <Icon
                className={cn('size-4', active ? 'text-accent-fg' : 'text-content-muted')}
                strokeWidth={1.75}
                aria-hidden="true"
              />
              <span className="max-[959px]:sr-only">{tab.label}</span>
            </button>
          )
        })}
      </nav>
      <button
        type="button"
        aria-label={t('navigation:shell.search')}
        title={paletteShortcut ? `${t('navigation:shell.search')} (${paletteShortcut})` : undefined}
        className={cn(
          'mx-auto flex h-8 min-w-0 shrink basis-[380px] items-center gap-2 rounded-sm bg-surface-muted pr-1.5 pl-2.5 text-sm text-content-muted ring-1 ring-stroke ring-inset transition-colors duration-(--d-fast) hover:text-content-secondary focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus max-[1279px]:w-8 max-[1279px]:shrink-0 max-[1279px]:basis-auto max-[1279px]:justify-center max-[1279px]:px-0',
          TOUCH_HIT_AREA,
        )}
        onClick={onOpenPalette}
      >
        <Search className="size-4 flex-none" aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate text-left max-[1279px]:hidden">
          {t('navigation:shell.search')}
        </span>
        {paletteShortcut && (
          <Kbd aria-hidden="true" className="max-[1279px]:hidden">
            {paletteShortcut}
          </Kbd>
        )}
      </button>
      {status}
      {fleet}
      <Button
        className="ml-1.5 flex-none"
        title={
          newTaskShortcut
            ? t('navigation:shell.newTaskShortcut', { shortcut: newTaskShortcut })
            : undefined
        }
        onClick={onNewTask}
      >
        <Plus aria-hidden="true" />
        <span className="max-[959px]:sr-only">{t('navigation:shell.newTask')}</span>
        {newTaskShortcut && (
          <Kbd
            aria-hidden="true"
            className="border-transparent bg-transparent text-current opacity-70 max-[1179px]:hidden"
          >
            {newTaskShortcut}
          </Kbd>
        )}
      </Button>
    </header>
  )
}
