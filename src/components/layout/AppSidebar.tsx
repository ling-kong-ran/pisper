// 工作台导航只负责展示；会话创建、搜索与设置跳转由应用壳传入。
import { lazy, Suspense, useMemo } from 'react'
import {
  ArrowLeft,
  Download,
  ExternalLink,
  RefreshCw,
  Rocket,
  Settings,
  MessageCirclePlus,
  Plug,
  Search,
  type LucideIcon,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import {
  getSettingsNavigation,
  SETTINGS_PAGES,
  settingsNavigationKey,
  type SettingsDestination,
} from '@/app/settings-navigation'
import { Sidebar as ShadcnSidebar, useSidebar } from '@/components/ui/sidebar'
import { Button } from '@/components/ui/button'
import { WorkbenchSidebarToggle } from './WorkbenchSidebarToggle'
import { useShortcutLabel } from '@/lib/shortcuts'
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { cn } from '@/lib/utils'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'

const SidebarMoreTools = lazy(() => import('@/components/layout/SidebarMoreTools'))

const SidebarRecentSessions = lazy(() =>
  import('@/components/layout/SidebarRecentSessions').then((m) => ({
    default: m.SidebarRecentSessions,
  })),
)

type SidebarUpdate = {
  info?: { desktop?: boolean; mobile?: boolean }
  status?: {
    state: string
    percent?: number
    availableVersion?: string
    behindBy?: number
    branch?: string
    availableCommit?: string
  }
}

type AppSidebarProps = {
  page: string
  configSection: string
  navigation: Array<[string, Array<[string, string, LucideIcon]>]>
  navigate: (page: string) => void
  navigateSettings: (destination: SettingsDestination) => void
  onExitSettings: () => void
  onNewChat: () => void
  onSearch: () => void
  onToggleTerminal?: () => void
  pluginStats?: { enabled: number; total: number } | null
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
  onToggleTerminal,
  pluginStats,
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
  const workspaceItems = navigation
    .flatMap(([, items]) => items)
    .filter(([id]) => id === 'schedules')
  if (runtimeFeatureAvailable(capabilities, 'plugins')) {
    workspaceItems.push(['plugins', t('navigation:workbench.plugins'), Plug])
  }
  const extraItems = navigation
    .flatMap(([, items]) => items)
    .filter(([id]) => !['chat', 'schedules', 'plugins'].includes(id))
  const runAndClose = (action: () => void) => {
    action()
    if (isMobile) setOpenMobile(false)
  }
  const navButton =
    'h-8 w-full justify-start gap-2 rounded-lg px-2.5 text-[14px] font-normal text-foreground shadow-none hover:bg-sidebar-accent'

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
              {workspaceItems.map(([id, , Icon]) => (
                <Button
                  key={id}
                  variant="ghost"
                  className={cn(navButton, page === id && 'bg-sidebar-accent')}
                  title={
                    id === 'plugins' && pluginStats
                      ? `${pluginStats.enabled} / ${pluginStats.total}`
                      : undefined
                  }
                  aria-current={page === id ? 'page' : undefined}
                  onClick={() => runAndClose(() => navigate(id))}
                >
                  <Icon size={16} />
                  <span>
                    {id === 'schedules'
                      ? t('navigation:workbench.automations')
                      : t('navigation:workbench.plugins')}
                  </span>
                </Button>
              ))}
              {(extraItems.length > 0 || onToggleTerminal) && (
                <Suspense fallback={null}>
                  <SidebarMoreTools
                    items={extraItems}
                    buttonClassName={navButton}
                    onNavigate={(id) => runAndClose(() => navigate(id))}
                    onTerminal={onToggleTerminal ? () => runAndClose(onToggleTerminal) : undefined}
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
          <SidebarUpdateStatus update={update} collapsed={collapsed} onOpen={onOpenUpdates} />
          <div className="flex h-10 items-center gap-2.5">
            <span
              aria-hidden="true"
              className="grid size-7 shrink-0 place-items-center rounded-full bg-foreground text-[12px] font-medium text-background"
            >
              P
            </span>
            <span className="min-w-0 flex-1 truncate text-[13px] font-medium">Pisper</span>
            <Button
              variant="ghost"
              size="icon-sm"
              title={t('navigation:navigation.settings')}
              aria-label={t('navigation:navigation.settings')}
              onClick={() => runAndClose(() => navigate('config'))}
            >
              <Settings size={16} />
            </Button>
          </div>
        </footer>
      </aside>
    </ShadcnSidebar>
  )
}

function SidebarUpdateStatus({
  update,
  collapsed,
  onOpen,
}: {
  update: SidebarUpdate
  collapsed: boolean
  onOpen: () => void
}) {
  const { t } = useI18n()
  const status = update?.status || { state: 'idle' }
  const nativeApp = Boolean(update?.info?.desktop || update?.info?.mobile)
  if (!['available', 'downloading', 'downloaded'].includes(status.state)) return null
  const downloading = status.state === 'downloading'
  const downloaded = status.state === 'downloaded'
  const label = downloaded
    ? t('navigation:appSidebar.readyToRestart')
    : downloading
      ? t('navigation:appSidebar.downloading')
      : nativeApp
        ? t('navigation:appSidebar.updateAvailable')
        : t('navigation:appSidebar.sourceUpdatesAvailable')
  const detail = downloading
    ? `${Math.round(status.percent || 0)}%`
    : nativeApp && status.availableVersion
      ? `v${status.availableVersion}`
      : status.behindBy
        ? t('navigation:appSidebar.countCommitsBehindBranch', {
            branch: status.branch || 'main',
            count: status.behindBy,
          })
        : status.availableCommit
          ? status.availableCommit.slice(0, 7)
          : t('navigation:appSidebar.viewUpdateDetails')
  const Icon = downloaded ? Rocket : downloading ? RefreshCw : nativeApp ? Download : ExternalLink

  return (
    <button
      type="button"
      className={`flex min-h-11 w-full items-center rounded-[var(--r-sm)] border border-[var(--stroke)] bg-[var(--accent-soft)] text-[var(--text)] transition-colors hover:bg-sidebar-accent ${collapsed ? 'justify-center px-0' : 'gap-2.5 px-3 text-left'}`}
      title={`${label} · ${detail}`}
      aria-label={`${label} · ${detail}`}
      onClick={onOpen}
    >
      <Icon className={downloading ? 'animate-spin shrink-0' : 'shrink-0'} size={16} />
      {!collapsed && (
        <span className="min-w-0">
          <strong className="block truncate text-[12px]">{label}</strong>
          <small className="mt-0.5 block truncate text-[11px] text-[var(--text-muted)]">
            {detail}
          </small>
        </span>
      )}
    </button>
  )
}
