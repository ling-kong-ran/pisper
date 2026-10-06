// 工作台导航只负责展示；会话创建、搜索与设置跳转由应用壳传入。
import { lazy, Suspense, useMemo } from 'react'
import { Blocks, Home, MessageCirclePlus, Search, type LucideIcon } from 'lucide-react'
import { APP_NAME } from '@/app/brand'
import { useI18n } from '@/app/i18n/use-i18n'
import type { Notify } from '@/app/routes/route-context'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import {
  getSettingsNavigation,
  settingsNavigationKey,
  SETTINGS_PAGES,
  type SettingsDestination,
} from '@/app/routes/settings-navigation'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { Sidebar as ShadcnSidebar, useSidebar } from '@/components/ui/sidebar'
import { Button } from '@/components/ui/button'
import { BrandLogo } from '@/components/common/BrandLogo'
import { WorkbenchSidebarToggle } from './WorkbenchSidebarToggle'
import { useShortcutLabel } from '@/lib/ui/shortcuts'
import { cn } from '@/lib/utils'

const SidebarSettingsButton = lazy(() =>
  import('@/components/layout/SidebarSettingsButton').then((module) => ({
    default: module.SidebarSettingsButton,
  })),
)

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
  installedTools: Array<{ id: string; name: string }>
  navigate: (page: string) => void
  onOpenComponent: (id: string) => void
  navigateSettings: (destination: SettingsDestination) => void
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
  installedTools,
  navigate,
  onOpenComponent,
  navigateSettings,
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
  const compact = collapsed && !isMobile
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
  const extraItems: Array<[string, string, LucideIcon]> = [
    ...items.filter(([id]) => !['chat', 'workflows', 'assets'].includes(id)),
    ...installedTools.map(({ id, name }): [string, string, LucideIcon] => [
      `component:${id}`,
      name,
      Blocks,
    ]),
  ]
  const runAndClose = (action: () => void) => {
    action()
    if (isMobile) setOpenMobile(false)
  }
  const navButton = cn(
    'w-full gap-2 rounded-lg text-sm font-normal text-foreground shadow-none hover:bg-sidebar-accent',
    compact ? 'h-11 justify-center px-0' : 'h-8 justify-start px-2.5',
  )
  const navLabel = compact ? 'sr-only' : 'truncate'

  return (
    <ShadcnSidebar collapsible="icon" className="pisper-sidebar-container border-0">
      <aside
        className="sidebar flex h-full w-full min-w-0 flex-col bg-sidebar text-foreground"
        id="workbench-sidebar"
        data-testid="workbench-sidebar"
      >
        <div
          className={cn(
            'flex h-12 shrink-0 items-center gap-2 px-3',
            compact ? 'justify-center' : 'justify-between',
          )}
          data-window-drag-region
        >
          <div className="flex min-w-0 items-center gap-2">
            <BrandLogo className="size-6" size={24} />
            <span className={cn(compact ? 'sr-only' : 'truncate', 'text-sm font-semibold')}>
              {APP_NAME}
            </span>
          </div>
        </div>
        {settingsActive ? (
          <nav
            className="min-h-0 flex-1 overflow-y-auto px-2 pb-4 pt-3"
            aria-label={t('config:settingsShell.settingsNavigation')}
          >
            <Button
              variant="ghost"
              className={cn(navButton, 'mb-5')}
              onClick={() => runAndClose(() => navigate('chat'))}
              data-testid="workbench-home"
              title={compact ? t('navigation:workbench.home') : undefined}
            >
              <Home size={16} aria-hidden="true" />
              <span className={navLabel}>{t('navigation:workbench.home')}</span>
            </Button>
            {settingsNavigation.map((group) => (
              <div key={group.label} className="mb-5 flex flex-col gap-0.5">
                <span
                  className={
                    compact ? 'sr-only' : 'px-2.5 pb-1.5 text-[12px] text-muted-foreground'
                  }
                >
                  {group.label}
                </span>
                {group.items.map((item) => (
                  <Button
                    key={item.key}
                    variant="ghost"
                    className={cn(navButton, activeSettingsKey === item.key && 'bg-sidebar-accent')}
                    aria-current={activeSettingsKey === item.key ? 'page' : undefined}
                    onClick={() => runAndClose(() => navigateSettings(item.destination))}
                    title={compact ? item.label : undefined}
                  >
                    <item.icon size={16} />
                    <span className={navLabel}>{item.label}</span>
                  </Button>
                ))}
              </div>
            ))}
          </nav>
        ) : (
          <>
            <nav
              className="flex min-h-0 flex-col gap-1 overflow-y-auto px-2 py-3"
              aria-label={t('navigation:appSidebar.mainNavigation')}
            >
              <Button
                variant="ghost"
                className={cn(navButton, page === 'chat' && 'bg-sidebar-accent')}
                aria-current={page === 'chat' ? 'page' : undefined}
                onClick={() => runAndClose(() => navigate('chat'))}
                data-testid="workbench-home"
                title={compact ? t('navigation:workbench.home') : undefined}
              >
                <Home size={16} aria-hidden="true" />
                <span className={navLabel}>{t('navigation:workbench.home')}</span>
              </Button>
              <Button
                variant="ghost"
                className={navButton}
                onClick={() => runAndClose(onNewChat)}
                data-testid="workbench-new-task"
                title={compact ? t('navigation:workbench.newTask') : undefined}
              >
                <MessageCirclePlus size={16} />
                <span className={navLabel}>{t('navigation:workbench.newTask')}</span>
                <kbd
                  className={cn(
                    'ml-auto text-[10px] font-normal text-muted-foreground/60',
                    compact && 'hidden',
                  )}
                >
                  {newChatShortcut}
                </kbd>
              </Button>
              <Button
                variant="ghost"
                className={navButton}
                onClick={() => runAndClose(onSearch)}
                title={compact ? t('navigation:workbench.search') : undefined}
              >
                <Search size={16} />
                <span className={navLabel}>{t('navigation:workbench.search')}</span>
                <kbd
                  className={cn(
                    'ml-auto text-[10px] font-normal text-muted-foreground/60',
                    compact && 'hidden',
                  )}
                >
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
                  title={compact ? label : undefined}
                >
                  <Icon size={16} />
                  <span className={navLabel}>{label}</span>
                </Button>
              ))}
              {extraItems.length > 0 && (
                <Suspense fallback={null}>
                  <SidebarMoreTools
                    items={extraItems}
                    buttonClassName={navButton}
                    compact={compact}
                    onNavigate={(id) =>
                      runAndClose(() =>
                        id.startsWith('component:') ? onOpenComponent(id.slice(10)) : navigate(id),
                      )
                    }
                  />
                </Suspense>
              )}
            </nav>
            <div className={cn('min-h-0 flex-1 flex-col', compact ? 'hidden' : 'flex')}>
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
        <footer
          className={cn(
            'mt-auto flex shrink-0 flex-col gap-2 pb-3 pt-2',
            compact ? 'px-2' : 'px-3',
          )}
        >
          {update.status &&
            ['available', 'downloading', 'downloaded'].includes(update.status.state) && (
              <Suspense fallback={null}>
                <SidebarUpdateStatus update={update} collapsed={compact} onOpen={onOpenUpdates} />
              </Suspense>
            )}
          <div className={cn('flex items-center', compact ? 'flex-col gap-1' : 'gap-2')}>
            <div className={cn('min-w-0', compact ? 'w-full' : 'flex-1')}>
              <Suspense fallback={<div className="h-11" aria-busy="true" />}>
                <SidebarSettingsButton
                  active={settingsActive}
                  compact={compact}
                  onOpen={() =>
                    runAndClose(() => navigateSettings({ type: 'config', id: 'models' }))
                  }
                />
              </Suspense>
            </div>
            <WorkbenchSidebarToggle inSidebar />
          </div>
        </footer>
      </aside>
    </ShadcnSidebar>
  )
}
