import { useEffect, useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  ChevronRight,
  Download,
  FolderClosed,
  ExternalLink,
  PanelLeftClose,
  PanelLeftOpen,
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
  requestSessionSelection,
} from '@/features/chat/events'
import { apiJson } from '@/lib/api'
import { relativeTime, workspaceName } from '@/lib/format'
import { SETTINGS_PAGES } from '@/features/config/SettingsShell'
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
  navigation: Array<[string, Array<[string, string, LucideIcon]>]>
  navigate: (page: string) => void
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
  navigation,
  navigate,
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

  const navigateFromSidebar = (id: string) => {
    navigate(id)
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
      <aside className={`sidebar shadcn-sidebar-content ${collapsed ? 'collapsed' : ''}`}>
        <button
          className="mobile-close"
          aria-label={t('navigation:appSidebar.closeNavigation')}
          onClick={() => setOpenMobile(false)}
        >
          <X size={18} />
        </button>
        <div className="nav-list">
          <nav className="nav-primary" aria-label={t('navigation:appSidebar.mainNavigation')}>
            {navigation.map(([group, items]) => (
              <div className="nav-group" key={group}>
                <span className="nav-group-label">{group}</span>
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
          <section
            className={`nav-history-section ${historyExpanded ? 'is-expanded' : ''}`}
            aria-label={t('navigation:appSidebar.recentChats')}
          >
            <div className="nav-history-section-head">
              <button
                className="nav-history-heading"
                aria-controls="sidebar-recent-sessions"
                aria-expanded={historyExpanded}
                onClick={() => setHistoryExpanded((value) => !value)}
              >
                <span>{t('navigation:appSidebar.recentChats')}</span>
                <ChevronRight className={historyExpanded ? 'is-open' : ''} size={14} />
              </button>
              <button
                className="nav-history-view-all"
                aria-label={t('navigation:appSidebar.viewAllCountChats', {
                  count: sessions.length,
                })}
                onClick={() => navigateFromSidebar('chatHistory')}
              >
                {t('navigation:appSidebar.viewAll')}
              </button>
            </div>
            {historyExpanded && (
              <div className="nav-history-list" id="sidebar-recent-sessions">
                {sessionGroups.map((group) => {
                  const groupCollapsed = collapsedWorkspaces.has(group.key)
                  return (
                    <div className="nav-workspace-group" key={group.key}>
                      <button
                        className="nav-workspace-heading"
                        aria-expanded={!groupCollapsed}
                        onClick={() => toggleWorkspace(group.key)}
                        title={group.cwd || t('navigation:appSidebar.noWorkspace')}
                      >
                        <ChevronRight className={groupCollapsed ? '' : 'is-open'} size={13} />
                        <FolderClosed size={13} />
                        <span>
                          {group.cwd
                            ? workspaceName(group.cwd, language)
                            : t('navigation:appSidebar.noWorkspace')}
                        </span>
                        <small>{group.sessions.length}</small>
                      </button>
                      {!groupCollapsed &&
                        group.sessions.map((session) => (
                          <button
                            className={`nav-history-item ${session.id === activeSessionId ? 'active-session' : ''}`}
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
                  <span className="nav-history-empty">
                    {t('navigation:appSidebar.noChatHistoryYet')}
                  </span>
                )}
              </div>
            )}
          </section>
        </div>
        <div className="mt-auto grid gap-2">
          <button
            className={`sidebar-settings ${settingsActive ? 'active' : ''}`}
            title={t('navigation:navigation.settings')}
            aria-current={settingsActive ? 'page' : undefined}
            onClick={() => navigateFromSidebar('config')}
          >
            <Settings size={16} />
            {!collapsed && <span>{t('navigation:navigation.settings')}</span>}
          </button>
          <SidebarUpdateStatus update={update} collapsed={collapsed} onOpen={onOpenUpdates} />
          <button
            className="sidebar-collapse !mt-0"
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
      <Icon className={downloading ? 'spin shrink-0' : 'shrink-0'} size={16} />
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
