// 侧边栏：工作台导航 + 设置分组导航 + 更新入口，支持折叠与移动端抽屉。
// 用 React Query 拉取 Provider/会话摘要等数据；折叠状态由 ui-store 持久化。
import { useEffect, useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  ArrowLeft,
  ChevronRight,
  Download,
  FolderClosed,
  ExternalLink,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  RefreshCw,
  Rocket,
  Settings,
  X,
  type LucideIcon,
} from 'lucide-react'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import {
  ACTIVE_SESSION_CHANGED_EVENT,
  SESSION_SELECTED_EVENT,
  SESSIONS_UPDATED_EVENT,
  requestSessionCreation,
  requestSessionSelection,
} from '@/features/chat/events'
import { apiJson } from '@/lib/api'
import { relativeTime, workspaceName } from '@/lib/format'
import {
  getSettingsNavigation,
  SETTINGS_PAGES,
  settingsNavigationKey,
  type SettingsDestination,
} from '@/app/settings-navigation'
import { Sidebar as ShadcnSidebar, useSidebar } from '@/components/ui/sidebar'

type SessionSummary = {
  id: string
  name?: string
  modified: string
  cwd?: string
}

type SidebarUpdate = {
  info?: { desktop?: boolean }
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
  onToggleCollapse: () => void
  update: SidebarUpdate
  onOpenUpdates: () => void
}

const RECENT_SESSION_LIMIT = 24

function workspaceKey(cwd = '') {
  const normalized = cwd.trim().replace(/\\/g, '/').replace(/\/+$/, '')
  return /^[A-Za-z]:\//.test(normalized) ? normalized.toLowerCase() : normalized
}

export function AppSidebar({
  page,
  configSection,
  navigation,
  navigate,
  navigateSettings,
  onExitSettings,
  collapsed,
  onToggleCollapse,
  update,
  onOpenUpdates,
}: AppSidebarProps) {
  const { t, language } = useI18n()
  const { isMobile, setOpenMobile } = useSidebar()
  const [historyExpanded, setHistoryExpanded] = useState(true)
  const [collapsedWorkspaces, setCollapsedWorkspaces] = useState<Set<string>>(() => new Set())
  const [activeSessionId, setActiveSessionId] = useState(
    () => localStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  const active = page === 'workflowCreate' ? 'workflows' : page === 'chatHistory' ? 'chat' : page
  const settingsActive = SETTINGS_PAGES.has(page)
  const settingsNavigation = useMemo(() => getSettingsNavigation(t), [t])
  const activeSettingsKey = settingsNavigationKey(page, configSection)

  const { data: sidebarSessionData, refetch: refreshSessions } = useQuery<{
    sessions: SessionSummary[]
  }>({
    queryKey: ['sessions', 'sidebar'],
    queryFn: () => apiJson<{ sessions: SessionSummary[] }>('/api/sessions'),
    refetchInterval: 20_000,
  })
  const sessions = useMemo(
    () =>
      [...(sidebarSessionData?.sessions || [])].sort(
        (a, b) => Date.parse(b.modified) - Date.parse(a.modified),
      ),
    [sidebarSessionData],
  )
  const sessionGroups = useMemo(() => {
    const groups = new Map<string, { key: string; cwd: string; sessions: SessionSummary[] }>()
    for (const session of sessions.slice(0, RECENT_SESSION_LIMIT)) {
      const key = workspaceKey(session.cwd) || '__no_workspace__'
      const group = groups.get(key) || { key, cwd: session.cwd || '', sessions: [] }
      group.sessions.push(session)
      groups.set(key, group)
    }
    return [...groups.values()]
  }, [sessions])

  useEffect(() => {
    const refresh = () => {
      void refreshSessions()
    }
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') refresh()
    }
    const syncActive = (event: Event) => {
      const detail = (event as CustomEvent<{ id?: string }>).detail
      setActiveSessionId(detail?.id || localStorage.getItem(STORAGE_KEYS.activeSession) || '')
    }
    document.addEventListener('visibilitychange', refreshWhenVisible)
    window.addEventListener(SESSIONS_UPDATED_EVENT, refresh)
    window.addEventListener(SESSION_SELECTED_EVENT, syncActive)
    window.addEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
    return () => {
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      window.removeEventListener(SESSIONS_UPDATED_EVENT, refresh)
      window.removeEventListener(SESSION_SELECTED_EVENT, syncActive)
      window.removeEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
    }
  }, [refreshSessions])

  const openRecentSession = (id: string) => {
    setActiveSessionId(id)
    requestSessionSelection(id)
    navigate('chat')
    if (isMobile) setOpenMobile(false)
  }

  const createSessionInWorkspace = (cwd: string) => {
    if (!requestSessionCreation(cwd)) return
    navigate('chat')
    if (isMobile) setOpenMobile(false)
  }

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

  const toggleWorkspace = (key: string) => {
    setCollapsedWorkspaces((current) => {
      const next = new Set(current)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  return (
    <ShadcnSidebar collapsible="icon" className="pisper-sidebar-container">
      <aside
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
          className={`nav-list [&_button]:relative [&_button]:flex [&_button]:w-full [&_button]:h-[34px] [&_button]:items-center [&_button]:gap-[10px] [&_button]:border-0 [&_button]:rounded-[var(--r-sm)] [&_button]:bg-transparent [&_button]:p-[0_10px] [&_button]:text-[var(--text-secondary)] [&_button]:text-left [&_button]:text-[12px] [&_button]:font-[500] [&_button]:[transition:var(--d1)_var(--ease-out)] [&_button:hover]:bg-[var(--surface-hover)] [&_button:hover]:text-[var(--text)] [&_button.active]:bg-[var(--star-soft)] [&_button.active]:text-[var(--text)] [&_button.active]:font-[600] [&_button.active::before]:[content:''] [&_button.active::before]:absolute [&_button.active::before]:left-[2px] [&_button.active::before]:top-[8px] [&_button.active::before]:bottom-[8px] [&_button.active::before]:w-[3px] [&_button.active::before]:rounded-[var(--r-pill)] [&_button.active::before]:bg-[var(--brand-blue)] min-[901px]:[.sidebar.collapsed_&_button]:justify-center min-[901px]:[.sidebar.collapsed_&_button]:gap-[0] min-[901px]:[.sidebar.collapsed_&_button]:p-0 min-[901px]:[.sidebar.collapsed_&_button_span]:hidden min-[901px]:[.sidebar.collapsed_&_button.active::before]:left-0 dark:[&_button.active]:bg-[var(--surface-hover)] min-[901px]:[[data-density='compact']_&_button]:h-[30px] flex min-h-0 flex-col gap-[3px] overflow-y-auto ${settingsActive ? 'nav-settings-mode gap-[10px]' : ''}`}
        >
          {settingsActive ? (
            <nav
              className="nav-primary [.nav-settings-mode_&]:gap-[0] flex flex-col gap-[3px]"
              aria-label={t('config:settingsShell.settingsNavigation')}
            >
              <button
                className="nav-settings-back [.nav-list_&]:mb-[13px] [.nav-list_&]:[border-bottom:1px_solid_var(--stroke-soft)] [.nav-list_&]:rounded-[0] [.nav-list_&]:p-[0_8px_10px] [.nav-list_&]:text-[var(--text)] [.nav-list_&]:font-[650] [.nav-list_&:hover]:bg-transparent [.nav-list_&:hover]:text-[var(--star-strong)]"
                title={t('navigation:appSidebar.backToWorkbench')}
                onClick={exitSettings}
              >
                <ArrowLeft size={16} />
                <span>{t('navigation:appSidebar.backToWorkbench')}</span>
              </button>
              {settingsNavigation.map((group) => (
                <div
                  className="nav-group [.nav-group_+_&]:mt-[10px] [.nav-settings-mode_.nav-group_+_&]:mt-[13px] min-[901px]:[.sidebar.collapsed_.nav-group_+_&]:mt-[8px] flex flex-col gap-[3px]"
                  key={group.label}
                >
                  <span className="nav-group-label min-[901px]:[.sidebar.collapsed_&]:hidden [padding:0_10px_4px] text-[var(--text-muted)] text-[11px] font-[700] tracking-[.12em]">
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
                  <span className="nav-group-label min-[901px]:[.sidebar.collapsed_&]:hidden [padding:0_10px_4px] text-[var(--text-muted)] text-[11px] font-[700] tracking-[.12em]">
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
            <section
              className={`nav-history-section min-[901px]:[.sidebar.collapsed_&]:hidden flex-1 min-h-0 flex flex-col [margin-top:10px] ${historyExpanded ? 'is-expanded' : ''}`}
              aria-label={t('navigation:appSidebar.recentChats')}
            >
              <div className="flex h-[34px] items-center justify-between gap-[6px] [padding:0_4px]">
                <button
                  className="nav-history-heading [.nav-list_&]:w-auto [.nav-list_&]:min-w-0 [.nav-list_&]:h-[28px] [.nav-list_&]:[flex:0_1_auto] [.nav-list_&]:gap-[4px] [.nav-list_&]:rounded-[var(--r-xs)] [.nav-list_&]:p-[0_6px] [.nav-list_&]:text-[var(--text-muted)] [.nav-list_&]:text-[11px] [.nav-list_&]:font-[600] [.nav-list_&:hover]:bg-transparent [.nav-list_&:hover]:text-[var(--text-secondary)] [&_>_span]:overflow-hidden [&_>_span]:text-ellipsis [&_>_span]:whitespace-nowrap [&_svg]:flex-none [&_svg]:[transition:transform_var(--d1)_var(--ease-out)] [&_svg.is-open]:[transform:rotate(90deg)]"
                  aria-controls="sidebar-recent-sessions"
                  aria-expanded={historyExpanded}
                  onClick={() => setHistoryExpanded((value) => !value)}
                >
                  <span>{t('navigation:appSidebar.recentChats')}</span>
                  <ChevronRight className={historyExpanded ? 'is-open' : ''} size={14} />
                </button>
                <button
                  className="nav-history-view-all [.nav-list_&]:w-auto [.nav-list_&]:h-[28px] [.nav-list_&]:flex-none [.nav-list_&]:rounded-[var(--r-xs)] [.nav-list_&]:p-[0_6px] [.nav-list_&]:text-[var(--text-muted)] [.nav-list_&]:text-[11px] [.nav-list_&]:font-[500] [.nav-list_&:hover]:bg-transparent [.nav-list_&:hover]:text-[var(--star-strong)]"
                  aria-label={t('navigation:appSidebar.viewAllCountChats', {
                    count: sessions.length,
                  })}
                  onClick={() => navigateFromSidebar('chatHistory')}
                >
                  {t('navigation:appSidebar.viewAll')}
                </button>
              </div>
              {historyExpanded && (
                <div
                  className="flex flex-1 min-h-0 flex-col gap-[2px] [padding-bottom:2px] overflow-y-auto [animation:page-in_var(--d1)_var(--ease-out)]"
                  id="sidebar-recent-sessions"
                >
                  {sessionGroups.map((group) => {
                    const groupCollapsed = collapsedWorkspaces.has(group.key)
                    const workspaceLabel = group.cwd
                      ? workspaceName(group.cwd, language)
                      : t('navigation:appSidebar.noWorkspace')
                    return (
                      <div
                        className="nav-workspace-group [.nav-workspace-group_+_&]:mt-[3px]"
                        key={group.key}
                      >
                        <div className="group/workspace flex min-w-0 items-center gap-[2px]">
                          <button
                            className="nav-workspace-heading [.nav-list_&]:grid [.nav-list_&]:w-auto [.nav-list_&]:min-w-0 [.nav-list_&]:h-[29px] [.nav-list_&]:min-h-[29px] [.nav-list_&]:flex-1 [.nav-list_&]:grid-cols-[13px_13px_minmax(0,1fr)_auto] [.nav-list_&]:items-center [.nav-list_&]:gap-[6px] [.nav-list_&]:p-[0_8px] [.nav-list_&]:text-[var(--text-muted)] [.nav-list_&]:text-[11px] [.nav-list_&]:font-[650] [.nav-list_&:hover]:bg-transparent [.nav-list_&:hover]:text-[var(--text)] [&_svg:first-child]:[transition:transform_var(--d1)_var(--ease-out)] [&_svg:first-child.is-open]:[transform:rotate(90deg)] [&_span]:overflow-hidden [&_span]:text-ellipsis [&_span]:whitespace-nowrap [&_small]:!text-[10px] [&_small]:[font-variant-numeric:tabular-nums]"
                            aria-expanded={!groupCollapsed}
                            onClick={() => toggleWorkspace(group.key)}
                            title={group.cwd || workspaceLabel}
                          >
                            <ChevronRight className={groupCollapsed ? '' : 'is-open'} size={13} />
                            <FolderClosed size={13} />
                            <span>{workspaceLabel}</span>
                            <small>{group.sessions.length}</small>
                          </button>
                          {group.cwd && (
                            <button
                              type="button"
                              className="nav-workspace-create [.nav-list_&]:grid [.nav-list_&]:w-[28px] [.nav-list_&]:h-[28px] [.nav-list_&]:min-h-[28px] [.nav-list_&]:flex-none [.nav-list_&]:place-items-center [.nav-list_&]:rounded-[var(--r-xs)] [.nav-list_&]:p-0 [.nav-list_&]:text-[var(--text-muted)] [.nav-list_&:hover]:bg-[var(--surface-hover)] [.nav-list_&:hover]:text-[var(--text)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--focus)] focus-visible:ring-inset"
                              title={t('navigation:appSidebar.newChatInWorkspace', {
                                workspace: group.cwd,
                              })}
                              aria-label={t('navigation:appSidebar.newChatInWorkspace', {
                                workspace: group.cwd,
                              })}
                              onClick={() => createSessionInWorkspace(group.cwd)}
                            >
                              <Plus size={14} />
                            </button>
                          )}
                        </div>
                        {!groupCollapsed &&
                          group.sessions.map((session) => (
                            <button
                              className={`nav-history-item [.nav-list_&]:flex [.nav-list_&]:w-full [.nav-list_&]:h-[34px] [.nav-list_&]:min-h-[34px] [.nav-list_&]:rounded-[var(--r-sm)] [.nav-list_&]:p-[0_8px_0_40px] [.nav-list_&]:text-[var(--text-secondary)] [.nav-list_&]:text-[12px] [.nav-list_&]:font-[500] [&_>_span]:overflow-hidden [&_>_span]:text-ellipsis [&_>_span]:whitespace-nowrap [.nav-list_&:hover]:bg-[var(--surface-muted)] [.nav-list_&:hover]:text-[var(--text)] min-[901px]:[[data-density='compact']_.nav-list_&]:h-[32px] min-[901px]:[[data-density='compact']_.nav-list_&]:min-h-[32px] ${session.id === activeSessionId ? 'active-session [.nav-list_.nav-history-item&]:bg-[var(--surface-muted)] [.nav-list_.nav-history-item&]:text-[var(--text)] [.nav-list_.nav-history-item&]:shadow-[inset_2px_0_var(--brand-blue)]' : ''}`}
                              aria-current={session.id === activeSessionId ? 'page' : undefined}
                              title={`${session.name || t('navigation:appSidebar.untitledChat')} · ${relativeTime(session.modified, language)}`}
                              onClick={() => openRecentSession(session.id)}
                              key={session.id}
                            >
                              <span>{session.name || t('navigation:appSidebar.untitledChat')}</span>
                            </button>
                          ))}
                      </div>
                    )
                  })}
                  {!sessions.length && (
                    <span className="[padding:8px] text-[var(--text-muted)] text-[11px]">
                      {t('navigation:appSidebar.noChatHistoryYet')}
                    </span>
                  )}
                </div>
              )}
            </section>
          )}
        </div>
        <div className="mt-auto grid gap-2">
          {!settingsActive && (
            <button
              className="sidebar-settings hover:bg-[var(--surface-hover)] hover:text-[var(--text)] [&.active]:bg-[var(--surface-hover)] [&.active]:text-[var(--text)] [&.active_svg]:text-[var(--brand-blue)] min-[901px]:[.sidebar.collapsed_&]:justify-center min-[901px]:[.sidebar.collapsed_&]:gap-[0] min-[901px]:[.sidebar.collapsed_&]:p-0 min-[901px]:[.sidebar.collapsed_&_span]:hidden flex w-full h-[34px] flex-none items-center gap-[9px] border-0 rounded-[var(--r-sm)] bg-transparent [padding:0_10px] text-[var(--text-muted)] text-[12px] font-[600] text-left"
              title={t('navigation:navigation.settings')}
              onClick={() => navigateFromSidebar('config')}
            >
              <Settings size={16} />
              {!collapsed && <span>{t('navigation:navigation.settings')}</span>}
            </button>
          )}
          <SidebarUpdateStatus update={update} collapsed={collapsed} onOpen={onOpenUpdates} />
          <button
            className="sidebar-collapse hover:bg-[var(--surface-hover)] hover:text-[var(--text)] min-[901px]:[.sidebar.collapsed_&]:justify-center min-[901px]:[.sidebar.collapsed_&]:gap-[0] min-[901px]:[.sidebar.collapsed_&]:p-0 min-[901px]:[.sidebar.collapsed_&_span]:hidden max-[900px]:hidden flex h-[34px] flex-none items-center gap-[8px] [margin-top:auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-transparent [padding:0_10px] text-[var(--text-muted)] text-[12px] cursor-pointer [transition:var(--d1)_var(--ease-out)] !mt-0"
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
            {collapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
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
  const desktop = Boolean(update?.info?.desktop)
  if (!['available', 'downloading', 'downloaded'].includes(status.state)) return null
  const downloading = status.state === 'downloading'
  const downloaded = status.state === 'downloaded'
  const label = downloaded
    ? t('navigation:appSidebar.readyToRestart')
    : downloading
      ? t('navigation:appSidebar.downloading')
      : desktop
        ? t('navigation:appSidebar.updateAvailable')
        : t('navigation:appSidebar.sourceUpdatesAvailable')
  const detail = downloading
    ? `${Math.round(status.percent || 0)}%`
    : desktop && status.availableVersion
      ? `v${status.availableVersion}`
      : status.behindBy
        ? t('navigation:appSidebar.countCommitsBehindBranch', {
            branch: status.branch || 'main',
            count: status.behindBy,
          })
        : status.availableCommit
          ? status.availableCommit.slice(0, 7)
          : t('navigation:appSidebar.viewUpdateDetails')
  const Icon = downloaded ? Rocket : downloading ? RefreshCw : desktop ? Download : ExternalLink

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
          <strong className="block truncate text-[12px]">{label}</strong>
          <small className="mt-0.5 block truncate text-[11px] text-[var(--text-muted)]">
            {detail}
          </small>
        </span>
      )}
    </button>
  )
}
