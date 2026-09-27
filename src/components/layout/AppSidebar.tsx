// 工作台导航只负责展示；会话创建、搜索与设置跳转由应用壳传入。
import { lazy, Suspense, useMemo } from 'react'
import { ArrowLeft, MessageCirclePlus, Search, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import {
  getSettingsNavigation,
  settingsNavigationKey,
  SETTINGS_PAGES,
  type SettingsDestination,
} from '@/app/settings-navigation'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { Sidebar as ShadcnSidebar, useSidebar } from '@/components/ui/sidebar'
import { Button } from '@/components/ui/button'
import { WorkbenchSidebarToggle } from './WorkbenchSidebarToggle'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'

const SidebarAccountMenu = lazy(() => import('@/components/layout/SidebarAccountMenu'))

const SidebarMoreTools = lazy(() => import('@/components/layout/SidebarMoreTools'))

const SidebarRecentSessions = lazy(() =>
  import('@/components/layout/SidebarRecentSessions').then((m) => ({
    default: m.SidebarRecentSessions,
  })),
)

import type { SidebarUpdate } from './SidebarUpdateStatus'
const SidebarUpdateStatus = lazy(() => import('./SidebarUpdateStatus'))

type AppSidebarProps = {
  page: string
  configSection: string
  navigation: Array<[string, Array<[string, string, LucideIcon]>]>
  navigate: (page: string) => void
  navigateSettings: (destination: SettingsDestination) => void
  onExitSettings: () => void
  onNewChat: () => void
  onSearch: () => void
  collapsed: boolean
  update: SidebarUpdate
  onOpenUpdates: () => void
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  notify: Notify
}

export function AppSidebar({
  page,
  configSection,
  navigation,
  navigate,
  navigateSettings,
  onExitSettings,
  onNewChat,
  onSearch,
  collapsed,
  update,
  onOpenUpdates,
  requestText,
  requestConfirm,
  notify,
}: AppSidebarProps) {
  const { t } = useI18n()
  const { isMobile, setOpenMobile } = useSidebar()
  const mobileApp = useIsMobileApp()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const settingsActive = SETTINGS_PAGES.has(page)
  const settingsNavigation = useMemo(
    () => getSettingsNavigation(t, { mobileApp, capabilities }),
    [capabilities, mobileApp, t],
  )
  const activeSettingsKey = settingsNavigationKey(page, configSection)
  const newChatShortcut = useShortcutLabel('primaryAction')
  const searchShortcut = useShortcutLabel('commandPalette')
  // 沿用壳层已过滤的能力清单；没有后端支持的入口不会出现在工作台。
  const items = navigation.flatMap(([, entries]) => entries)
  const workspaceItems = ['workflows', 'assets'].flatMap((id) =>
    items.filter(([key]) => key === id),
  )
  const extraItems = items.filter(([id]) => !['chat', 'workflows', 'assets'].includes(id))
  const runAndClose = (action: () => void) => {
    action()
    if (isMobile) setOpenMobile(false)
  }
  const navButton =
    'h-8 w-full justify-start gap-2 rounded-lg px-2.5 text-sm font-normal text-foreground shadow-none hover:bg-sidebar-accent'

  return (
    <ShadcnSidebar collapsible="offcanvas" className="pisper-sidebar-container border-0">
      <aside
        className="sidebar flex h-full w-full min-w-0 flex-col bg-sidebar text-foreground"
        data-testid="workbench-sidebar"
      >
        <div className="flex h-12 shrink-0 items-center px-3" data-window-drag-region>
          <WorkbenchSidebarToggle inSidebar />
        </div>
        {settingsActive ? (
          <nav
            className="min-h-0 flex-1 overflow-y-auto px-2 pb-4"
            aria-label={t('config:settingsShell.settingsNavigation')}
          >
            <Button
              variant="ghost"
              className={cn(navButton, 'mb-5')}
              onClick={() => runAndClose(onExitSettings)}
            >
              <ArrowLeft size={16} />
              <span>{t('navigation:appSidebar.backToApp')}</span>
            </Button>
            {settingsNavigation.map((group) => (
              <div key={group.label} className="mb-5 flex flex-col gap-0.5">
                <span className="px-2.5 pb-1.5 text-[12px] text-muted-foreground">
                  {group.label}
                </span>
                {group.items.map((item) => (
                  <Button
                    key={item.key}
                    variant="ghost"
                    className={cn(navButton, activeSettingsKey === item.key && 'bg-sidebar-accent')}
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
            <nav
              className="flex shrink-0 flex-col gap-1 px-2 py-3"
              aria-label={t('navigation:appSidebar.mainNavigation')}
            >
              <Button
                variant="ghost"
                className={navButton}
                onClick={() => runAndClose(onNewChat)}
                data-testid="workbench-new-task"
              >
                <MessageCirclePlus size={16} />
                <span>{t('navigation:workbench.newTask')}</span>
                <kbd className="ml-auto text-[10px] font-normal text-muted-foreground/60">
                  {newChatShortcut}
                </kbd>
              </Button>
              <Button variant="ghost" className={navButton} onClick={() => runAndClose(onSearch)}>
                <Search size={16} />
                <span>{t('navigation:workbench.search')}</span>
                <kbd className="ml-auto text-[10px] font-normal text-muted-foreground/60">
                  {searchShortcut}
                </kbd>
              </Button>
              {workspaceItems.map(([id, label, Icon]) => (
                <Button
                  key={id}
                  variant="ghost"
                  className={cn(navButton, page === id && 'bg-sidebar-accent')}
                  aria-current={page === id ? 'page' : undefined}
                  onClick={() => runAndClose(() => navigate(id))}
                >
                  <Icon size={16} />
                  <span>{label}</span>
                </Button>
              ))}
              {extraItems.length > 0 && (
                <Suspense fallback={null}>
                  <SidebarMoreTools
                    items={extraItems}
                    buttonClassName={navButton}
                    onNavigate={(id) => runAndClose(() => navigate(id))}
                  />
                </Suspense>
              )}
            </nav>
            <div className="flex min-h-0 flex-1 flex-col">
              <Suspense
                fallback={<div className="mx-2 h-20 animate-pulse rounded-lg bg-sidebar-accent" />}
              >
                <SidebarRecentSessions
                  navigate={navigate}
                  requestText={requestText}
                  requestConfirm={requestConfirm}
                  notify={notify}
                />
              </Suspense>
            </div>
          </>
        )}
        <footer className="flex shrink-0 flex-col gap-2 px-3 pb-3 pt-2">
          {update.status &&
            ['available', 'downloading', 'downloaded'].includes(update.status.state) && (
              <Suspense fallback={null}>
                <SidebarUpdateStatus update={update} collapsed={collapsed} onOpen={onOpenUpdates} />
              </Suspense>
            )}
          <Suspense fallback={<div className="h-11" aria-busy="true" />}>
            <SidebarAccountMenu
              onProvider={() =>
                runAndClose(() => navigateSettings({ type: 'config', id: 'models' }))
              }
              onAppearance={() =>
                runAndClose(() => navigateSettings({ type: 'config', id: 'interface' }))
              }
            />
          </Suspense>
        </footer>
      </aside>
    </ShadcnSidebar>
  )
}
