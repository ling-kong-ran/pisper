// 导航外壳只接收项目列表插槽；任务数据和操作归聊天领域所有。
import { lazy, Suspense, useMemo, type ReactNode } from 'react'
import { flushSync } from 'react-dom'
import { Home, SquarePen, Search, X, PanelLeftClose } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  getSettingsNavigation,
  settingsNavigationKey,
  SETTINGS_PAGES,
  type SettingsDestination,
} from '@/app/settings-navigation'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { Sidebar, useSidebar } from '@/components/ui/sidebar'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { SidebarResizeHandle } from './SidebarResizeHandle'
import type { SidebarUpdate } from './SidebarUpdateStatus'

const SidebarUpdateStatus = lazy(() => import('./SidebarUpdateStatus'))
// 快捷键提示依赖持久化的快捷键 Store，懒加载以免把它带进启动包。
const SidebarShortcutHint = lazy(() => import('./SidebarShortcutHint'))

type AppSidebarProps = {
  page: string
  // 插件等页可从功能栏分组进入；由壳层按进入来源判定是否归设置，缺省时按页面集合判断。
  settingsOwned?: boolean
  configSection: string
  navigate: (page: string) => void
  navigateSettings: (destination: SettingsDestination) => void
  onNewChat: () => void
  onSearch: () => void
  update: SidebarUpdate
  onOpenUpdates: () => void
  tasks: ReactNode
  settingsSearch?: ReactNode
  width?: number
  onWidthPreview?: (width: number | null) => void
  onWidthCommit?: (width: number) => void
}

export function AppSidebar({
  page,
  settingsOwned,
  configSection,
  navigate,
  navigateSettings,
  onNewChat,
  onSearch,
  update,
  onOpenUpdates,
  tasks,
  settingsSearch,
  width,
  onWidthPreview,
  onWidthCommit,
}: AppSidebarProps) {
  const { t } = useI18n()
  const { isMobile, setOpenMobile, open, setOpen, toggleSidebar } = useSidebar()
  const mobileApp = useIsMobileApp()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const settingsNavigation = useMemo(
    () => getSettingsNavigation(t, { mobileApp, capabilities }),
    [capabilities, mobileApp, t],
  )
  // 「关于」与版本号固定在设置导航底部，其余分区保持分组顺序。
  const aboutItem = settingsNavigation
    .flatMap((group) => group.items)
    .find((item) => item.key === 'config:about')
  const settingsGroups = settingsNavigation
    .map((group) => ({ ...group, items: group.items.filter((item) => item !== aboutItem) }))
    .filter((group) => group.items.length > 0)
  const settingsActive = settingsOwned ?? SETTINGS_PAGES.has(page)
  const activeSettingsKey = settingsNavigationKey(page, configSection)
  const runAndClose = (action: () => void) => {
    action()
    if (isMobile) setOpenMobile(false)
  }
  // 行高 36px、16px 图标、正文字号跟随界面字号（不取整），紧凑密度收到 30px；
  // 选中行抬起为键帽并带 3px 强调色刻度，不只依赖底色差异表达选中。
  const navButton =
    'relative h-9 w-full justify-start gap-2.5 rounded-sm border-0 px-2.5 text-[length:var(--app-font-size)] font-normal text-content-secondary shadow-none hover:bg-surface-hover hover:text-content in-data-[density=compact]:h-[30px] pointer-coarse:h-11 [&_svg]:text-content-muted'
  const navButtonActive =
    'bg-key font-medium text-content shadow-key ring-1 ring-key-border hover:bg-key [&_svg]:text-accent-fg before:absolute before:top-1/2 before:left-0 before:h-4 before:w-[3px] before:-translate-y-1/2 before:rounded-r-[3px] before:bg-star'
  return (
    <Sidebar
      collapsible={settingsActive && !isMobile ? 'none' : 'offcanvas'}
      className="pisper-sidebar-container overflow-hidden border-0"
    >
      <aside
        id="workbench-sidebar"
        data-testid="workbench-sidebar"
        className={cn(
          'relative flex h-full w-full min-w-0 flex-col overflow-hidden bg-sidebar text-foreground',
          // 右侧 12px 沟槽留给宽度调整把手，避免把手压住会话行尾的时间。
          !isMobile && 'pr-1',
        )}
      >
        <div className="flex h-12 shrink-0 items-center gap-1 pr-2 pl-4" data-window-drag-region>
          <span className="min-w-0 flex-1 truncate text-[15px] font-semibold tracking-[-.01em]">
            {settingsActive
              ? t('navigation:navigation.settings')
              : t('navigation:rail.workspaceShort')}
          </span>
          {!isMobile && open && !settingsActive && (
            <Button
              id="workbench-sidebar-toggle"
              variant="ghost"
              size="icon"
              // 展开态的 aria-expanded 不应让图标键常驻按下底色，只在悬停时显示。
              className="text-content-muted aria-expanded:bg-transparent aria-expanded:text-content-muted aria-expanded:hover:bg-surface-hover aria-expanded:hover:text-content"
              aria-label={t('navigation:appSidebar.collapseSidebar')}
              title={t('navigation:appSidebar.collapseSidebar')}
              aria-expanded={open}
              aria-controls="workbench-sidebar"
              onClick={() => {
                flushSync(toggleSidebar)
                document.getElementById('workbench-sidebar-toggle')?.focus()
              }}
            >
              <PanelLeftClose />
            </Button>
          )}
          {isMobile && (
            <Button
              variant="ghost"
              className="ml-auto size-11"
              aria-label={t('navigation:appSidebar.collapseSidebar')}
              onClick={() => setOpenMobile(false)}
            >
              <X />
            </Button>
          )}
        </div>
        {settingsSearch && <div className="relative z-20 shrink-0 px-2 pb-2">{settingsSearch}</div>}
        {settingsActive ? (
          <nav
            className="min-h-0 flex-1 overflow-y-auto px-2 pb-3"
            aria-label={t('config:settingsShell.settingsNavigation')}
          >
            <Button
              variant="ghost"
              className={cn(navButton, 'mb-1')}
              onClick={() => runAndClose(() => navigate('chat'))}
              data-testid="workbench-home"
            >
              <Home size={16} />
              <span className="truncate">{t('navigation:workbench.home')}</span>
            </Button>
            {settingsGroups.map((group) => (
              <div key={group.label} className="mt-3 flex flex-col gap-0.5">
                <span className="flex h-7 items-center px-2.5 text-xs font-medium tracking-[.02em] text-content-muted">
                  {group.label}
                </span>
                {group.items.map((item) => (
                  <Button
                    key={item.key}
                    variant="ghost"
                    className={cn(navButton, activeSettingsKey === item.key && navButtonActive)}
                    aria-current={activeSettingsKey === item.key ? 'page' : undefined}
                    onClick={() => runAndClose(() => navigateSettings(item.destination))}
                  >
                    <item.icon size={16} />
                    <span className="truncate">{item.label}</span>
                  </Button>
                ))}
              </div>
            ))}
          </nav>
        ) : (
          <>
            <div className="flex shrink-0 flex-col gap-1 px-2 pb-1">
              {/* 新任务是侧栏唯一的键帽按钮；搜索平铺，二者都带快捷键键帽提示。 */}
              <Button
                variant="secondary"
                className="h-[34px] w-full min-w-0 justify-start gap-2 border-key-border px-2.5 text-[13px] max-[650px]:h-11 pointer-coarse:h-11 [&_svg]:text-content-secondary"
                onClick={() => runAndClose(onNewChat)}
                data-testid="workbench-new-task"
              >
                <SquarePen size={16} />
                <span className="truncate">{t('navigation:workbench.newTask')}</span>
                <Suspense fallback={null}>
                  <SidebarShortcutHint action="primaryAction" />
                </Suspense>
              </Button>
              <Button
                variant="ghost"
                className="h-8 w-full min-w-0 justify-start gap-2 px-2.5 text-[13px] font-normal text-content-muted max-[650px]:h-11 pointer-coarse:h-11"
                title={t('navigation:workbench.search')}
                onClick={() => runAndClose(onSearch)}
              >
                <Search size={16} />
                <span className="truncate">{t('navigation:workbench.search')}</span>
                <Suspense fallback={null}>
                  <SidebarShortcutHint action="commandPalette" />
                </Suspense>
              </Button>
            </div>
            <div className="flex min-h-0 flex-1 flex-col">{tasks}</div>
          </>
        )}
        {settingsActive && aboutItem && (
          <div className="shrink-0 px-2 pb-2">
            <Button
              variant="ghost"
              className={cn(navButton, activeSettingsKey === aboutItem.key && navButtonActive)}
              aria-current={activeSettingsKey === aboutItem.key ? 'page' : undefined}
              onClick={() => runAndClose(() => navigateSettings(aboutItem.destination))}
            >
              <aboutItem.icon size={16} />
              <span className="truncate">{aboutItem.label}</span>
              {update.info?.version && (
                <small className="ml-auto font-mono text-xs font-normal text-content-muted tabular-nums">
                  v{update.info.version}
                </small>
              )}
            </Button>
          </div>
        )}
        {update.status &&
          ['available', 'downloading', 'downloaded'].includes(update.status.state) && (
            <footer className="shrink-0 px-2 pb-2">
              <Suspense fallback={null}>
                <SidebarUpdateStatus update={update} collapsed={false} onOpen={onOpenUpdates} />
              </Suspense>
            </footer>
          )}
        {!isMobile && width !== undefined && onWidthPreview && onWidthCommit && (
          <SidebarResizeHandle
            width={width}
            label={t('navigation:appSidebar.resizeSidebar')}
            hint={
              settingsActive
                ? t('navigation:appSidebar.resizeSidebarHint')
                : t('navigation:appSidebar.resizeSidebarCollapseHint')
            }
            onPreview={onWidthPreview}
            onCommit={onWidthCommit}
            onCollapse={settingsActive ? undefined : () => setOpen(false)}
          />
        )}
      </aside>
    </Sidebar>
  )
}
