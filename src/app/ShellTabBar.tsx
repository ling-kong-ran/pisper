// 手机底部标签栏（只在 phoneShell 下挂载）：会话 · 看板 · [居中新任务] · 更多 · 设置（主页是会话；
// “组件”挪进“更多”的磁贴）。“看板”标签在任何页面都带琥珀实心的需要你计数徽标，离开看板也不会错过等你的事项。
// 键盘弹起时整栏隐藏，避免压住输入框；会话页与主页整栏不渲染（看板入口在顶栏左侧，返回走系统返回键）。
// 选中只用强调色文字与顶边刻度，不加底色块。
import { useEffect } from 'react'
import { LayoutGrid, Plus, Settings, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { PHONE_SHELL_TABS, ShellTab } from '@/app/shell-tabs'
import { LaneBarsIcon, SessionBubbleIcon } from '@/components/app/LaneBarsIcon'
import { useFleetSummary } from '@/features/board/public'
import { cn } from '@/lib/utils'

type PhoneTab = (typeof PHONE_SHELL_TABS)[number]

const TAB_ICONS: Record<PhoneTab, LucideIcon> = {
  session: SessionBubbleIcon,
  board: LaneBarsIcon,
  more: LayoutGrid,
  settings: Settings,
}

type ShellTabBarProps = {
  // 会话页不显示标签栏，但仍订阅舰队计量，让外壳的 ⌥A 目标保持最新。
  hidden?: boolean
  activeTab: ShellTab | null
  onSelectTab: (tab: ShellTab) => void
  onNewTask: () => void
  // 与标题栏计量相同：等待最久事项变化时通知外壳，⌥A（外接键盘）由外壳统一处理。
  onLongestWaitingChange?: (sessionId: string) => void
}

export function ShellTabBar({
  hidden = false,
  activeTab,
  onSelectTab,
  onNewTask,
  onLongestWaitingChange,
}: ShellTabBarProps) {
  const { t } = useI18n()
  const fleet = useFleetSummary()
  const attention = fleet.attention
  const longestId = fleet.longestWaiting?.sessionId || ''
  useEffect(() => onLongestWaitingChange?.(longestId), [longestId, onLongestWaitingChange])
  if (hidden) return null
  const labels: Record<PhoneTab, string> = {
    session: t('navigation:navigation.chat'),
    board: t('navigation:shell.board'),
    more: t('navigation:shell.more'),
    settings: t('navigation:navigation.settings'),
  }
  const tab = (id: PhoneTab) => {
    const Icon = TAB_ICONS[id]
    const active = activeTab === id
    const badge = id === 'board' && attention > 0
    return (
      <button
        key={id}
        type="button"
        data-shell-tab={id}
        aria-current={active ? 'page' : undefined}
        aria-label={
          badge
            ? `${labels[id]}, ${t('navigation:shell.tabBarAttention', { count: attention })}`
            : undefined
        }
        className={cn(
          'relative flex h-14 min-w-0 flex-col items-center justify-center gap-1 border-0 bg-transparent px-1 text-xs leading-none text-content-muted transition-colors duration-(--d-fast) focus-visible:ring-2 focus-visible:ring-focus focus-visible:outline-none focus-visible:ring-inset active:bg-surface-subtle',
          active &&
            'font-medium text-accent-fg before:absolute before:top-[-1px] before:left-1/2 before:h-0.5 before:w-7 before:-translate-x-1/2 before:rounded-b-[2px] before:bg-star',
        )}
        onClick={() => onSelectTab(id)}
      >
        <span className="relative">
          <Icon className="size-[22px]" strokeWidth={active ? 2 : 1.75} aria-hidden="true" />
          {badge && (
            <span
              aria-hidden="true"
              className="absolute -top-1.5 left-3.5 flex h-[18px] min-w-[18px] items-center justify-center rounded-full bg-st-attn-solid px-1 font-mono text-[11px] leading-none font-semibold text-on-attn ring-1 ring-st-attn-ring tabular-nums"
            >
              {attention > 99 ? '99+' : attention}
            </span>
          )}
        </span>
        <span className="max-w-full truncate">{labels[id]}</span>
      </button>
    )
  }
  return (
    <nav
      aria-label={t('navigation:shell.primaryNavigation')}
      data-testid="mobile-tab-bar"
      className="mobile-tab-bar grid flex-none grid-cols-5 items-center border-t border-stroke bg-panel pb-[var(--pisper-safe-area-bottom)] [[data-mobile-keyboard='open']_&]:hidden"
    >
      {tab('session')}
      {tab('board')}
      <div className="flex h-14 items-center justify-center">
        <button
          type="button"
          aria-label={t('navigation:shell.newTask')}
          className="flex h-10 w-[52px] items-center justify-center rounded-md bg-star text-on-accent shadow-primary transition-colors duration-(--d-fast) hover:bg-star-hover focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
          onClick={onNewTask}
        >
          <Plus className="size-6" strokeWidth={2} aria-hidden="true" />
        </button>
      </div>
      {tab('more')}
      {tab('settings')}
    </nav>
  )
}
