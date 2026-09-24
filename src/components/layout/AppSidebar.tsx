// 侧边栏：应用导航 + 设置分组导航 + 更新入口，支持折叠与移动端抽屉。
// 用 React Query 拉取 Provider 等数据；折叠状态由 ui-store 持久化。
// 「最近会话」区块（含右键新建/删除项目）拆到 SidebarRecentSessions 并懒加载，
// 避免目录选择弹窗与右键菜单原语进入应用壳的 eager 入口（受打包预算约束）。
import { lazy, Suspense, useMemo } from 'react'
import {
  ArrowLeft,
  Download,
  ExternalLink,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  RefreshCw,
  Rocket,
  Settings,
  X,
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
import { useIsMobileApp } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

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
  collapsed: boolean
  side?: 'left' | 'right'
  width?: number
  onToggleCollapse: () => void
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
  collapsed,
  side = 'left',
  width,
  onToggleCollapse,
  update,
  onOpenUpdates,
  requestText,
  requestConfirm,
  notify,
}: AppSidebarProps) {
  const { t } = useI18n()
  const { isMobile, setOpenMobile } = useSidebar()
  const active = page === 'workflowCreate' ? 'workflows' : page === 'chatHistory' ? 'chat' : page
  const settingsActive = SETTINGS_PAGES.has(page)
  const mobileApp = useIsMobileApp()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const settingsNavigation = useMemo(
    () => getSettingsNavigation(t, { mobileApp, capabilities }),
    [capabilities, mobileApp, t],
  )
  const activeSettingsKey = settingsNavigationKey(page, configSection)

  const navigateFromSidebar = (id: string) => {
    navigate(id)
    if (isMobile) setOpenMobile(false)
  }

  const navigateSettingsFromSidebar = (destination: SettingsDestination) => {
    navigateSettings(destination)
    if (isMobile) setOpenMobile(false)
  }

  const exitSettings = () => {
    onExitSettings()
    if (isMobile) setOpenMobile(false)
  }

  return (
    <ShadcnSidebar side={side} collapsible="icon" className="pisper-sidebar-container">
      <aside
        style={!isMobile && !collapsed && width ? { width, minWidth: width } : undefined}
        className={`sidebar dark:border-[var(--stroke)] dark:bg-[var(--sidebar-bg)] dark:shadow-[0_14px_40px_-18px_var(--sidebar-shadow)] max-[1150px]:w-[205px] max-[1150px]:min-w-[205px] max-[1150px]:p-[16px_14px] max-[900px]:fixed max-[900px]:inset-[0_auto_0_0] max-[900px]:w-[236px] max-[900px]:[transform:translateX(-102%)] max-[900px]:[transition:transform_var(--d2)_var(--ease-out)] max-[900px]:[&.is-open]:[transform:translateX(0)] relative z-[30] w-[236px] min-w-[236px] h-full flex flex-col gap-[18px] [padding:18px] [border-right:1px_solid_var(--stroke)] bg-[var(--sidebar-bg)] shadow-[0_14px_40px_-18px_var(--sidebar-shadow)] shadcn-sidebar-content max-[900px]:[.sidebar&]:relative max-[900px]:[.sidebar&]:inset-[auto] max-[900px]:[.sidebar&]:w-[236px] max-[900px]:[.sidebar&]:min-w-[236px] max-[900px]:[.sidebar&]:[transform:none] max-[900px]:[.sidebar&]:[transition:none] ${collapsed ? "collapsed min-[901px]:[.sidebar&]:w-[64px] min-[901px]:[.sidebar&]:min-w-[64px] min-[901px]:[.sidebar&]:gap-[12px] min-[901px]:[.sidebar&]:p-[14px_10px] min-[901px]:[[data-density='compact']_.sidebar:not(&)]:w-[218px] min-[901px]:[[data-density='compact']_.sidebar:not(&)]:min-w-[218px] min-[901px]:[[data-density='compact']_.sidebar:not(&)]:p-[14px]" : ''}`}
      >
        <button
          className="mobile-close hover:bg-[var(--surface-hover)] hover:text-[var(--text)] max-[900px]:grid max-[900px]:place-items-center hidden w-[32px] h-[32px] flex-none [margin-left:auto] border-0 rounded-[var(--r-sm)] bg-transparent text-[var(--text-muted)] cursor-pointer"
          aria-label={t('navigation:appSidebar.closeNavigation')}
          onClick={() => setOpenMobile(false)}
        >
          <X size={18} />
        </button>
        <div
          className={`nav-list [&_button]:relative [&_button]:flex [&_button]:w-full [&_button]:h-[34px] [&_button]:items-center [&_button]:gap-[10px] [&_button]:border-0 [&_button]:rounded-[var(--r-sm)] [&_button]:bg-transparent [&_button]:p-[0_10px] [&_button]:text-[var(--text-secondary)] [&_button]:text-left [&_button]:text-[length:var(--app-font-size)] [&_button]:font-medium [&_button]:[transition:var(--d1)_var(--ease-out)] [&_button:hover]:bg-[var(--surface-hover)] [&_button:hover]:text-[var(--text)] [&_button.active]:bg-[var(--star-soft)] [&_button.active]:text-[var(--text)] [&_button.active::before]:[content:''] [&_button.active::before]:absolute [&_button.active::before]:left-[2px] [&_button.active::before]:top-[8px] [&_button.active::before]:bottom-[8px] [&_button.active::before]:w-[3px] [&_button.active::before]:rounded-[var(--r-pill)] [&_button.active::before]:bg-[var(--brand-blue)] min-[901px]:[.sidebar.collapsed_&_button]:justify-center min-[901px]:[.sidebar.collapsed_&_button]:gap-[0] min-[901px]:[.sidebar.collapsed_&_button]:p-0 min-[901px]:[.sidebar.collapsed_&_button_span]:hidden min-[901px]:[.sidebar.collapsed_&_button.active::before]:left-0 dark:[&_button.active]:bg-[var(--surface-hover)] min-[901px]:[[data-density='compact']_&_button]:h-[30px] flex min-h-0 flex-col gap-[3px] overflow-y-auto ${settingsActive ? 'nav-settings-mode gap-[10px]' : ''}`}
        >
          {settingsActive ? (
            <nav
              className="nav-primary [.nav-settings-mode_&]:gap-[0] flex flex-col gap-[3px]"
              aria-label={t('config:settingsShell.settingsNavigation')}
            >
              <button
                className="nav-settings-back [.nav-list_&]:mb-[13px] [.nav-list_&]:[border-bottom:1px_solid_var(--stroke-soft)] [.nav-list_&]:rounded-[0] [.nav-list_&]:p-[0_8px_10px] [.nav-list_&]:text-[var(--text)] [.nav-list_&]:font-medium [.nav-list_&:hover]:bg-transparent [.nav-list_&:hover]:text-[var(--star-strong)]"
                title={t('navigation:appSidebar.backToApp')}
                onClick={exitSettings}
              >
                <ArrowLeft size={16} />
                <span>{t('navigation:appSidebar.backToApp')}</span>
              </button>
              {settingsNavigation.map((group) => (
                <div
                  className="nav-group [.nav-group_+_&]:mt-[10px] [.nav-settings-mode_.nav-group_+_&]:mt-[13px] min-[901px]:[.sidebar.collapsed_.nav-group_+_&]:mt-[8px] flex flex-col gap-[3px]"
                  key={group.label}
                >
                  <span className="nav-group-label min-[901px]:[.sidebar.collapsed_&]:hidden [padding:0_10px_4px] text-[var(--text-muted)] text-[12px] font-medium tracking-normal">
                    {group.label}
                  </span>
                  {group.items.map((item) => {
                    const Icon = item.icon
                    const isActive = activeSettingsKey === item.key
                    return (
                      <button
                        className={`nav-main ${isActive ? 'active' : ''}`}
                        aria-current={isActive ? 'page' : undefined}
                        key={item.key}
                        title={item.label}
                        onClick={() => navigateSettingsFromSidebar(item.destination)}
                      >
                        <Icon size={16} />
                        <span>{item.label}</span>
                      </button>
                    )
                  })}
                </div>
              ))}
            </nav>
          ) : (
            <nav
              className="nav-primary [.nav-settings-mode_&]:gap-[0] flex flex-col gap-[3px]"
              aria-label={t('navigation:appSidebar.mainNavigation')}
            >
              {navigation.map(([group, items]) => (
                <div
                  className="nav-group [.nav-group_+_&]:mt-[10px] [.nav-settings-mode_.nav-group_+_&]:mt-[13px] min-[901px]:[.sidebar.collapsed_.nav-group_+_&]:mt-[8px] flex flex-col gap-[3px]"
                  key={group}
                >
                  <span className="nav-group-label min-[901px]:[.sidebar.collapsed_&]:hidden [padding:0_10px_4px] text-[var(--text-muted)] text-[12px] font-medium tracking-normal">
                    {group}
                  </span>
                  {items.map(([id, label, Icon]) => (
                    <button
                      className={`nav-main ${active === id ? 'active' : ''}`}
                      key={id}
                      title={label}
                      onClick={() => navigateFromSidebar(id)}
                    >
                      <Icon size={16} />
                      <span>{label}</span>
                    </button>
                  ))}
                </div>
              ))}
            </nav>
          )}
          {!settingsActive && (
            <Suspense fallback={null}>
              <SidebarRecentSessions
                navigate={navigate}
                requestText={requestText}
                requestConfirm={requestConfirm}
                notify={notify}
              />
            </Suspense>
          )}
        </div>
        <div className="mt-auto grid gap-2">
          {!settingsActive && (
            <button
              className="sidebar-settings hover:bg-[var(--surface-hover)] hover:text-[var(--text)] [&.active]:bg-[var(--surface-hover)] [&.active]:text-[var(--text)] [&.active_svg]:text-[var(--brand-blue)] min-[901px]:[.sidebar.collapsed_&]:justify-center min-[901px]:[.sidebar.collapsed_&]:gap-[0] min-[901px]:[.sidebar.collapsed_&]:p-0 min-[901px]:[.sidebar.collapsed_&_span]:hidden flex w-full h-[34px] flex-none items-center gap-[9px] border-0 rounded-[var(--r-sm)] bg-transparent [padding:0_10px] text-[var(--text-muted)] text-[length:var(--app-font-size)] font-medium text-left"
              title={t('navigation:navigation.settings')}
              onClick={() => navigateFromSidebar('config')}
            >
              <Settings size={16} />
              {!collapsed && <span>{t('navigation:navigation.settings')}</span>}
            </button>
          )}
          <SidebarUpdateStatus update={update} collapsed={collapsed} onOpen={onOpenUpdates} />
          <button
            className="sidebar-collapse hover:bg-[var(--surface-hover)] hover:text-[var(--text)] min-[901px]:[.sidebar.collapsed_&]:justify-center min-[901px]:[.sidebar.collapsed_&]:gap-[0] min-[901px]:[.sidebar.collapsed_&]:p-0 min-[901px]:[.sidebar.collapsed_&_span]:hidden max-[900px]:hidden flex h-[34px] flex-none items-center gap-[8px] [margin-top:auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-transparent [padding:0_10px] text-[var(--text-muted)] text-[length:var(--app-font-size)] font-medium cursor-pointer [transition:var(--d1)_var(--ease-out)] !mt-0"
            title={
              collapsed
                ? t('navigation:appSidebar.expandSidebar')
                : t('navigation:appSidebar.collapseSidebar')
            }
            aria-label={
              collapsed
                ? t('navigation:appSidebar.expandSidebar')
                : t('navigation:appSidebar.collapseSidebar')
            }
            onClick={onToggleCollapse}
          >
            {side === 'right' ? (
              collapsed ? (
                <PanelRightOpen size={16} />
              ) : (
                <PanelRightClose size={16} />
              )
            ) : collapsed ? (
              <PanelLeftOpen size={16} />
            ) : (
              <PanelLeftClose size={16} />
            )}
            <span>
              {collapsed
                ? t('navigation:appSidebar.expandSidebar')
                : t('navigation:appSidebar.collapseSidebar')}
            </span>
          </button>
        </div>
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
      className={`flex min-h-11 w-full items-center rounded-[var(--r-sm)] border border-[var(--stroke)] bg-[var(--accent-soft)] text-[var(--text)] transition-colors hover:bg-[var(--surface-hover)] ${collapsed ? 'justify-center px-0' : 'gap-2.5 px-3 text-left'}`}
      title={`${label} · ${detail}`}
      aria-label={`${label} · ${detail}`}
      onClick={onOpen}
    >
      <Icon className={downloading ? 'animate-spin shrink-0' : 'shrink-0'} size={16} />
      {!collapsed && (
        <span className="min-w-0">
          <strong className="block truncate text-[length:var(--app-font-size)] font-medium">
            {label}
          </strong>
          <small className="mt-0.5 block truncate text-[12px] font-normal text-[var(--text-muted)]">
            {detail}
          </small>
        </span>
      )}
    </button>
  )
}
