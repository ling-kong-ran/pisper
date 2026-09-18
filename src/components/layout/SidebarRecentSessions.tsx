// 侧边栏「最近会话」区块：搜索、按工作区分组的会话列表与右键菜单
//（空白处新建项目/删除项目、分组上新建会话/删除项目）。
// 从 AppSidebar 拆出并懒加载：应用壳属于 eager 入口、受打包预算约束，
// 目录选择弹窗与 radix 右键菜单原语只在区块内按需加载。
import { lazy, Suspense, useEffect, useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  ChevronRight,
  FolderClosed,
  FolderPlus,
  MessageSquare,
  Pencil,
  Plus,
  Trash2,
} from 'lucide-react'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import {
  ACTIVE_SESSION_CHANGED_EVENT,
  SESSION_SELECTED_EVENT,
  announceActiveSession,
  announceSessionsUpdated,
  requestSessionCreation,
  requestSessionSelection,
} from '@/features/chat/events'
import { fetchStartupQuery, startupQueryOptions } from '@/lib/startup-queries'
import { apiJson } from '@/lib/api'
import { relativeTime, workspaceName } from '@/lib/format'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from '@/components/ui/context-menu'
import { useSidebar } from '@/components/ui/sidebar'

// 目录选择器只在「新建项目」时用到：按需加载。
const WorkspacePicker = lazy(() =>
  import('@/components/WorkspacePicker').then((m) => ({ default: m.WorkspacePicker })),
)

type SessionSummary = {
  id: string
  name?: string
  modified: string
  cwd?: string
}

type SessionGroup = { key: string; cwd: string; sessions: SessionSummary[] }

type SidebarRecentSessionsProps = {
  navigate: (page: string) => void
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  notify: Notify
}

const RECENT_SESSION_LIMIT = 24

function workspaceKey(cwd = '') {
  const normalized = cwd.trim().replace(/\\/g, '/').replace(/\/+$/, '')
  return /^[A-Za-z]:\//.test(normalized) ? normalized.toLowerCase() : normalized
}

// 极简相对时间（ZCode 式列表标注）：刚刚 / 18分 / 1时 / 3天 / 9/12。
function shortRelativeTime(value: string, locale = 'zh-CN') {
  if (!value) return ''
  const distance = Date.now() - new Date(value).getTime()
  if (distance < 60_000) return locale === 'en-US' ? 'now' : '刚刚'
  if (distance < 3_600_000) {
    const minutes = Math.floor(distance / 60_000)
    return locale === 'en-US' ? `${minutes}m` : `${minutes}分`
  }
  if (distance < 86_400_000) {
    const hours = Math.floor(distance / 3_600_000)
    return locale === 'en-US' ? `${hours}h` : `${hours}时`
  }
  if (distance < 7 * 86_400_000) {
    const days = Math.floor(distance / 86_400_000)
    return locale === 'en-US' ? `${days}d` : `${days}天`
  }
  return new Date(value).toLocaleDateString(locale, { month: 'numeric', day: 'numeric' })
}

export function SidebarRecentSessions({
  navigate,
  requestText,
  requestConfirm,
  notify,
}: SidebarRecentSessionsProps) {
  const { t, language } = useI18n()
  const { isMobile, setOpenMobile } = useSidebar()
  const [historyExpanded, setHistoryExpanded] = useState(true)
  const [collapsedWorkspaces, setCollapsedWorkspaces] = useState<Set<string>>(() => new Set())
  const [activeSessionId, setActiveSessionId] = useState(
    () => localStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  // 右键菜单状态：menuTargetKey 记录右键落在哪个工作区分组（空串=空白区域），
  // menuTargetSessionId 记录右键落点是否为某个会话（优先级高于分组）；
  // projectPickerOpen 控制新建项目的目录选择弹窗，deletingKey 防止重复提交删除。
  const [menuTargetKey, setMenuTargetKey] = useState('')
  const [menuTargetSessionId, setMenuTargetSessionId] = useState('')
  const [projectPickerOpen, setProjectPickerOpen] = useState(false)
  const [deletingKey, setDeletingKey] = useState('')

  const { data: sidebarSessionData } = useQuery({
    ...startupQueryOptions<{ sessions: SessionSummary[] }>('sessions'),
    refetchInterval: 20_000,
  })
  // 搜索框已按反馈移除（列表本身带分组与快捷键直达，搜索入口鸡肋）。
  const visibleSessions = useMemo(
    () =>
      [...(sidebarSessionData?.sessions || [])].sort(
        (a, b) => Date.parse(b.modified) - Date.parse(a.modified),
      ),
    [sidebarSessionData],
  )
  const sessionGroups = useMemo(() => {
    const groups = new Map<string, SessionGroup>()
    for (const session of visibleSessions.slice(0, RECENT_SESSION_LIMIT)) {
      const key = workspaceKey(session.cwd) || '__no_workspace__'
      const group = groups.get(key) || { key, cwd: session.cwd || '', sessions: [] }
      group.sessions.push(session)
      groups.set(key, group)
    }
    return [...groups.values()]
  }, [visibleSessions])
  const menuTargetGroup = sessionGroups.find((group) => group.key === menuTargetKey) || null
  const menuTargetSession =
    visibleSessions.find((session) => session.id === menuTargetSessionId) || null
  const workspaceLabel = (group: SessionGroup) =>
    group.cwd ? workspaceName(group.cwd, language) : t('navigation:appSidebar.noWorkspace')

  useEffect(() => {
    const refresh = () => {
      void fetchStartupQuery('sessions', true).catch(() => {})
    }
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') refresh()
    }
    const syncActive = (event: Event) => {
      const detail = (event as CustomEvent<{ id?: string }>).detail
      setActiveSessionId(detail?.id || localStorage.getItem(STORAGE_KEYS.activeSession) || '')
    }
    document.addEventListener('visibilitychange', refreshWhenVisible)
    window.addEventListener(SESSION_SELECTED_EVENT, syncActive)
    window.addEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
    return () => {
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      window.removeEventListener(SESSION_SELECTED_EVENT, syncActive)
      window.removeEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
    }
  }, [])

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

  // 删除执行体：逐个调用会话删除接口，命中活动会话时切换到剩余会话，最后广播刷新。
  const removeSessions = async (targets: SessionSummary[]) => {
    const deletedIds = new Set(targets.map((session) => session.id))
    for (const session of targets) {
      await apiJson(`/api/sessions/${encodeURIComponent(session.id)}`, { method: 'DELETE' })
    }
    if (deletedIds.has(activeSessionId)) {
      const nextId = visibleSessions.find((session) => !deletedIds.has(session.id))?.id || ''
      setActiveSessionId(nextId)
      if (nextId) localStorage.setItem(STORAGE_KEYS.activeSession, nextId)
      else localStorage.removeItem(STORAGE_KEYS.activeSession)
      announceActiveSession(nextId)
    }
    announceSessionsUpdated()
    void fetchStartupQuery('sessions', true).catch(() => {})
  }

  // 删除项目：按侧边栏的语义，项目=同一工作目录下的会话分组；删除前确认。
  const deleteProject = async (group: SessionGroup) => {
    if (!group.sessions.length || deletingKey) return
    const label = workspaceLabel(group)
    const approved = await requestConfirm({
      title: t('navigation:appSidebar.deleteProject'),
      message: t('navigation:appSidebar.deleteProjectConfirm', {
        project: label,
        count: group.sessions.length,
      }),
      confirmLabel: t('navigation:appSidebar.deleteProjectAction'),
    })
    if (!approved) return
    setDeletingKey(group.key)
    try {
      await removeSessions(group.sessions)
      notify(t('navigation:appSidebar.projectDeleted', { project: label }))
    } catch (error) {
      notify(error instanceof Error ? error.message : String(error), 'error')
    } finally {
      setDeletingKey('')
    }
  }

  // 删除单个会话（右键会话菜单）。
  const deleteSingleSession = async (session: SessionSummary) => {
    if (deletingKey) return
    const name = session.name || t('navigation:appSidebar.untitledChat')
    const approved = await requestConfirm({
      title: t('chat:chatHistoryPage.deleteChat'),
      message: t('chat:chatHistoryPage.deleteChatNameThisAlsoRemovesLocalChatHistory', {
        name,
      }),
      confirmLabel: t('chat:chatHistoryPage.delete'),
    })
    if (!approved) return
    setDeletingKey(session.id)
    try {
      await removeSessions([session])
      notify(t('chat:chatHistoryPage.chatDeleted'))
    } catch (error) {
      notify(error instanceof Error ? error.message : String(error), 'error')
    } finally {
      setDeletingKey('')
    }
  }

  // 重命名会话（右键会话菜单）：弹输入框，成功后广播列表刷新。
  const renameSession = async (session: SessionSummary) => {
    const name = await requestText({
      title: t('chat:chatHistoryPage.renameChat'),
      inputLabel: t('chat:chatHistoryPage.chatTitle'),
      value: session.name,
      confirmLabel: t('chat:chatHistoryPage.save'),
    })
    if (name === null || name === session.name) return
    try {
      await apiJson(`/api/sessions/${encodeURIComponent(session.id)}`, {
        method: 'PATCH',
        body: JSON.stringify({ name }),
      })
      announceSessionsUpdated()
      void fetchStartupQuery('sessions', true).catch(() => {})
      notify(t('chat:chatHistoryPage.chatTitleUpdated'))
    } catch (error) {
      notify(error instanceof Error ? error.message : String(error), 'error')
    }
  }

  const toggleWorkspace = (key: string) => {
    setCollapsedWorkspaces((current) => {
      const next = new Set(current)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  const navigateFromSection = (id: string) => {
    navigate(id)
    if (isMobile) setOpenMobile(false)
  }

  return (
    <section
      className={`nav-history-section min-[901px]:[.sidebar.collapsed_&]:hidden flex-1 min-h-0 flex flex-col [margin-top:10px] ${historyExpanded ? 'is-expanded' : ''}`}
      aria-label={t('navigation:appSidebar.recentChats')}
    >
      <div className="flex h-[30px] items-center justify-between gap-[6px] [padding:0_4px]">
        {/* ZCode 式：标题收敛为单个箭头图标，点击开/合整个会话列表。 */}
        <button
          className="nav-history-heading grid h-[26px] w-[26px] place-items-center rounded-[var(--r-xs)] text-[var(--text-muted)] hover:bg-[var(--surface-hover)] hover:text-[var(--text)]"
          aria-controls="sidebar-recent-sessions"
          aria-expanded={historyExpanded}
          aria-label={t('navigation:appSidebar.recentChats')}
          title={t('navigation:appSidebar.recentChats')}
          onClick={() => setHistoryExpanded((value) => !value)}
        >
          <ChevronRight
            className={
              historyExpanded
                ? 'rotate-90 [transition:transform_var(--d1)_var(--ease-out)]'
                : '[transition:transform_var(--d1)_var(--ease-out)]'
            }
            size={14}
          />
        </button>
        <button
          className="nav-history-view-all h-[26px] flex-none rounded-[var(--r-xs)] border-0 bg-transparent p-[0_6px] text-[var(--text-muted)] text-[11px] font-[500] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--star-strong)]"
          aria-label={t('navigation:appSidebar.viewAllCountChats', {
            count: visibleSessions.length,
          })}
          onClick={() => navigateFromSection('chatHistory')}
        >
          {t('navigation:appSidebar.viewAll')}
        </button>
      </div>
      {historyExpanded && (
        <ContextMenu
          onOpenChange={(open) => {
            if (!open) {
              setMenuTargetKey('')
              setMenuTargetSessionId('')
            }
          }}
        >
          <ContextMenuTrigger asChild>
            <div
              className="flex flex-1 min-h-0 flex-col gap-[2px] [padding-bottom:2px] overflow-y-auto overflow-x-hidden [animation:page-in_var(--d1)_var(--ease-out)]"
              id="sidebar-recent-sessions"
            >
              {sessionGroups.map((group) => {
                const groupCollapsed = collapsedWorkspaces.has(group.key)
                const label = workspaceLabel(group)
                return (
                  <div
                    className="nav-workspace-group [.nav-workspace-group_+_&]:mt-[3px]"
                    key={group.key}
                  >
                    <div
                      className="group/workspace flex min-w-0 items-center gap-[2px]"
                      onContextMenu={() => setMenuTargetKey(group.key)}
                    >
                      <button
                        className="nav-workspace-heading [.nav-list_&]:grid [.nav-list_&]:w-auto [.nav-list_&]:min-w-0 [.nav-list_&]:h-[29px] [.nav-list_&]:min-h-[29px] [.nav-list_&]:flex-1 [.nav-list_&]:grid-cols-[13px_minmax(0,1fr)_auto] [.nav-list_&]:items-center [.nav-list_&]:gap-[6px] [.nav-list_&]:p-[0_8px] [.nav-list_&]:text-[var(--text-muted)] [.nav-list_&]:text-[11px] [.nav-list_&]:font-[650] [.nav-list_&:hover]:bg-transparent [.nav-list_&:hover]:text-[var(--text)] [&_span]:overflow-hidden [&_span]:text-ellipsis [&_span]:whitespace-nowrap [&_small]:!text-[10px] [&_small]:[font-variant-numeric:tabular-nums]"
                        aria-expanded={!groupCollapsed}
                        onClick={() => toggleWorkspace(group.key)}
                        title={group.cwd || label}
                      >
                        <FolderClosed size={13} />
                        <span>{label}</span>
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
                          className={`nav-history-item [.nav-list_&]:flex [.nav-list_&]:w-full [.nav-list_&]:h-[32px] [.nav-list_&]:min-h-[32px] [.nav-list_&]:rounded-[var(--r-xs)] [.nav-list_&]:p-[0_8px_0_24px] [.nav-list_&]:text-[var(--text-secondary)] [.nav-list_&]:text-[12px] [.nav-list_&]:font-[500] [.nav-list_&]:gap-[8px] [&_>_span]:flex-1 [&_>_span]:min-w-0 [&_>_span]:overflow-hidden [&_>_span]:text-ellipsis [&_>_span]:whitespace-nowrap [&_>_small]:flex-none [&_>_small]:shrink-0 [&_>_small]:text-[var(--text-muted)] [&_>_small]:text-[11px] [&_>_small]:font-[400] [.nav-list_&:hover]:bg-[var(--surface-muted)] [.nav-list_&:hover]:text-[var(--text)] [.nav-list_&:hover_>_small]:opacity-100 min-[901px]:[[data-density='compact']_.nav-list_&]:h-[30px] min-[901px]:[[data-density='compact']_.nav-list_&]:min-h-[30px] ${session.id === activeSessionId ? 'active-session [.nav-list_.nav-history-item&]:bg-[var(--surface-muted)] [.nav-list_.nav-history-item&]:text-[var(--text)] [.nav-list_.nav-history-item&]:shadow-[inset_2px_0_var(--brand-blue)]' : ''}`}
                          aria-current={session.id === activeSessionId ? 'page' : undefined}
                          title={`${session.name || t('navigation:appSidebar.untitledChat')} · ${relativeTime(session.modified, language)}`}
                          onClick={() => openRecentSession(session.id)}
                          onContextMenu={() => setMenuTargetSessionId(session.id)}
                          key={session.id}
                        >
                          <span className="select-none">
                            {session.name || t('navigation:appSidebar.untitledChat')}
                          </span>
                          <small className="opacity-70">
                            {shortRelativeTime(session.modified, language)}
                          </small>
                        </button>
                      ))}
                  </div>
                )
              })}
              {!visibleSessions.length && (
                <span className="[padding:8px] text-[var(--text-muted)] text-[11px]">
                  {t('navigation:appSidebar.noChatHistoryYet')}
                </span>
              )}
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent className="w-[220px]">
            {menuTargetSession ? (
              <>
                <ContextMenuItem onSelect={() => openRecentSession(menuTargetSession.id)}>
                  <MessageSquare size={13} />
                  {t('navigation:appSidebar.openSession')}
                </ContextMenuItem>
                <ContextMenuItem onSelect={() => void renameSession(menuTargetSession)}>
                  <Pencil size={13} />
                  {t('chat:chatHistoryPage.renameChat')}
                </ContextMenuItem>
                <ContextMenuItem
                  variant="destructive"
                  disabled={Boolean(deletingKey)}
                  onSelect={() => void deleteSingleSession(menuTargetSession)}
                >
                  <Trash2 size={13} />
                  {t('chat:chatHistoryPage.delete')}
                </ContextMenuItem>
              </>
            ) : menuTargetGroup ? (
              <>
                {menuTargetGroup.cwd && (
                  <ContextMenuItem onSelect={() => createSessionInWorkspace(menuTargetGroup.cwd)}>
                    <Plus size={13} />
                    {t('navigation:appSidebar.newChat')}
                  </ContextMenuItem>
                )}
                <ContextMenuItem
                  variant="destructive"
                  disabled={Boolean(deletingKey) || !menuTargetGroup.sessions.length}
                  onSelect={() => void deleteProject(menuTargetGroup)}
                >
                  <Trash2 size={13} />
                  {t('navigation:appSidebar.deleteProject')}
                </ContextMenuItem>
              </>
            ) : (
              <>
                <ContextMenuItem onSelect={() => setProjectPickerOpen(true)}>
                  <FolderPlus size={13} />
                  {t('navigation:appSidebar.newProject')}
                </ContextMenuItem>
                <ContextMenuSub>
                  <ContextMenuSubTrigger disabled={!sessionGroups.length || Boolean(deletingKey)}>
                    <Trash2 size={13} />
                    {t('navigation:appSidebar.deleteProject')}
                  </ContextMenuSubTrigger>
                  <ContextMenuSubContent className="max-w-[280px]">
                    {sessionGroups.map((group) => (
                      <ContextMenuItem key={group.key} onSelect={() => void deleteProject(group)}>
                        <span className="truncate">{workspaceLabel(group)}</span>
                      </ContextMenuItem>
                    ))}
                  </ContextMenuSubContent>
                </ContextMenuSub>
              </>
            )}
          </ContextMenuContent>
        </ContextMenu>
      )}
      {projectPickerOpen && (
        <Suspense fallback={null}>
          <WorkspacePicker
            open
            description={t('navigation:appSidebar.newProjectPickerDescription')}
            onOpenChange={setProjectPickerOpen}
            onSelect={(path) => {
              setProjectPickerOpen(false)
              createSessionInWorkspace(path)
            }}
          />
        </Suspense>
      )}
    </section>
  )
}
