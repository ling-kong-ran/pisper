// 导航外壳只接收项目列表插槽；任务数据和操作归聊天领域所有。
import { lazy, Suspense, useMemo, type ReactNode } from 'react'
import { flushSync } from 'react-dom'
import { Home, SquarePen, Search, X, PanelLeftClose } from 'lucide-react'
import { APP_NAME } from '@/app/brand'
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
import type { SidebarUpdate } from './SidebarUpdateStatus'

const SidebarUpdateStatus = lazy(() => import('./SidebarUpdateStatus'))

type AppSidebarProps = {
  page: string
  configSection: string
  navigate: (page: string) => void
  navigateSettings: (destination: SettingsDestination) => void
  onNewChat: () => void
  onSearch: () => void
  update: SidebarUpdate
  onOpenUpdates: () => void
  tasks: ReactNode
  settingsSearch?: ReactNode
}

export function AppSidebar({
  page,
  configSection,
  navigate,
  navigateSettings,
  onNewChat,
  onSearch,
  update,
  onOpenUpdates,
  tasks,
  settingsSearch,
}: AppSidebarProps) {
  const { t } = useI18n()
  const { isMobile, setOpenMobile, open, toggleSidebar, persistent } = useSidebar()
  const mobileApp = useIsMobileApp()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const settingsNavigation = useMemo(
    () => getSettingsNavigation(t, { mobileApp, capabilities }),
    [capabilities, mobileApp, t],
  )
  const settingsActive = SETTINGS_PAGES.has(page)
  const activeSettingsKey = settingsNavigationKey(page, configSection)
  const runAndClose = (action: () => void) => {
    action()
    if (isMobile) setOpenMobile(false)
  }
  const navButton =
    'h-9 w-full justify-start gap-2 rounded-lg px-2.5 text-sm font-normal shadow-none hover:bg-sidebar-accent'
  return (
    <Sidebar
      collapsible="offcanvas"
      className="pisper-sidebar-container overflow-hidden border-0 min-[651px]:rounded-l-2xl"
    >
      <aside
        id="workbench-sidebar"
        data-testid="workbench-sidebar"
        className="flex h-full w-full min-w-0 flex-col overflow-hidden bg-sidebar text-foreground min-[651px]:rounded-l-2xl"
      >
        <div className="flex h-14 shrink-0 items-center gap-1 px-3" data-window-drag-region>
          <span className="min-w-0 flex-1 truncate pl-1 text-lg font-semibold">
            {settingsActive ? t('navigation:navigation.settings') : APP_NAME}
          </span>
          {!settingsActive && (
            <Button
              variant="ghost"
              size="icon"
              aria-label={t('navigation:workbench.search')}
              title={t('navigation:workbench.search')}
              onClick={() => runAndClose(onSearch)}
            >
              <Search size={18} />
            </Button>
          )}
          {!isMobile && open && (
            <Button
              id="workbench-sidebar-toggle"
              variant="ghost"
              className="size-9 shrink-0"
              aria-label={t('navigation:appSidebar.collapseSidebar')}
              title={t('navigation:appSidebar.collapseSidebar')}
              aria-expanded={open}
              aria-controls="workbench-sidebar"
              onClick={() => {
                flushSync(toggleSidebar)
                document.getElementById('workbench-sidebar-toggle')?.focus()
              }}
            >
              <PanelLeftClose size={18} />
            </Button>
          )}
          {isMobile && !persistent && (
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
            className="min-h-0 flex-1 overflow-y-auto px-2 py-3"
            aria-label={t('config:settingsShell.settingsNavigation')}
          >
            <Button
              variant="ghost"
              className={cn(navButton, 'mb-4')}
              onClick={() => runAndClose(() => navigate('chat'))}
              data-testid="workbench-home"
            >
              <Home size={16} />
              {t('navigation:workbench.home')}
            </Button>
            {settingsNavigation.map((group) => (
              <div key={group.label} className="mb-5 flex flex-col gap-0.5">
                <span className="px-2.5 pb-1.5 text-xs text-muted-foreground">{group.label}</span>
                {group.items.map((item) => (
                  <Button
                    key={item.key}
                    variant="ghost"
                    className={cn(
                      navButton,
                      activeSettingsKey === item.key && 'bg-sidebar-accent font-medium',
                    )}
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
            <div className="flex shrink-0 items-center gap-1 px-2 py-2">
              <Button
                variant="ghost"
                className={cn(navButton, 'min-w-0 flex-1')}
                onClick={() => runAndClose(onNewChat)}
                data-testid="workbench-new-task"
              >
                <SquarePen size={18} />
                <span className="truncate">{t('navigation:workbench.newTask')}</span>
              </Button>
            </div>
            <div className="flex min-h-0 flex-1 flex-col">{tasks}</div>
          </>
        )}
        {update.status &&
          ['available', 'downloading', 'downloaded'].includes(update.status.state) && (
            <footer className="shrink-0 px-3 py-2">
              <Suspense fallback={null}>
                <SidebarUpdateStatus update={update} collapsed={false} onOpen={onOpenUpdates} />
              </Suspense>
            </footer>
          )}
      </aside>
    </Sidebar>
  )
}
