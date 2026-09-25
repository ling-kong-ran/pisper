// 侧边栏「最近会话」区块：搜索、按工作区分组的会话列表与右键菜单
//（空白处新建项目/删除项目、分组上新建会话/重命名项目/删除项目）。
// 从 AppSidebar 拆出并懒加载：应用壳属于 eager 入口、受打包预算约束，
// 目录选择弹窗与 radix 右键菜单原语只在区块内按需加载。
import {
  lazy,
  Suspense,
  useEffect,
  useMemo,
  useState,
  type PointerEvent,
  type ReactNode,
} from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Archive,
  ArchiveRestore,
  ChevronRight,
  CircleAlert,
  FolderClosed,
  FolderPlus,
  MessageSquare,
  Pencil,
  Pin,
  PinOff,
  Plus,
  RotateCcw,
  Search,
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
  subscribeSessionDeletionUpdates,
} from '@/features/chat/events'
import {
  SessionOrganizationProtocolError,
  updateSessionOrganization,
} from '@/features/chat/session-organization-api'
import { orderVisibleSessions } from '@/features/chat/session-list'
import {
  deleteSessionsSequentially,
  groupSessionsByWorkspace,
  orderWorkspaceGroups,
  recentWorkspaceGroups,
  replacementActiveSessionId,
  sessionWorkspaceKey,
  sessionsInWorkspace,
  type WorkspaceSessionGroup,
} from '@/features/chat/session-workspaces'
import { useWorkspaceOrderStore } from '@/features/chat/workspace-order-store'
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
import { Button } from '@/components/ui/button'

// 目录选择器只在「新建项目」时用到：按需加载。
const WorkspacePicker = lazy(() =>
  import('@/components/WorkspacePicker').then((m) => ({ default: m.WorkspacePicker })),
)

type SessionSummary = {
  id: string
  name?: string
  modified: string
  cwd?: string
  pinned?: boolean
  archived?: boolean
  unread?: boolean
  needsAttention?: boolean
}

type SessionGroup = WorkspaceSessionGroup<SessionSummary>

type SidebarRecentSessionsProps = {
  navigate: (page: string) => void
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  notify: Notify
}

const RECENT_SESSION_LIMIT = 24

function WorkspaceActionButton({
  label,
  onClick,
  children,
}: {
  label: string
  onClick: () => void
  children: ReactNode
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      className="opacity-0 group-hover/workspace:opacity-100 group-focus-within/workspace:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100 !size-7 !min-h-7 !shrink-0 !p-0 !text-[var(--text-muted)] hover:!bg-[var(--surface-hover)] hover:!text-[var(--text)] max-[900px]:!size-11 max-[900px]:!min-h-11"
      title={label}
      aria-label={label}
      onClick={onClick}
    >
      {children}
    </Button>
  )
}

export function SidebarRecentSessions({
  navigate,
  requestText,
  requestConfirm,
  notify,
}: SidebarRecentSessionsProps) {
  const { t, language } = useI18n()
  const queryClient = useQueryClient()
  const { isMobile, setOpenMobile } = useSidebar()
  const [historyExpanded, setHistoryExpanded] = useState(true)
  const [sessionQuery, setSessionQuery] = useState('')
  const [filterOpen, setFilterOpen] = useState(false)
  const [collapsedWorkspaces, setCollapsedWorkspaces] = useState<Set<string>>(() => new Set())
  const workspaceOrder = useWorkspaceOrderStore((state) => state.order)
  const rememberWorkspaces = useWorkspaceOrderStore((state) => state.rememberWorkspaces)
  const workspaceNames = useWorkspaceOrderStore((state) => state.names)
  const setWorkspaceName = useWorkspaceOrderStore((state) => state.setWorkspaceName)
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
  useEffect(
    () =>
      subscribeSessionDeletionUpdates(window, ({ deletedIds }) => {
        const deleted = new Set(deletedIds)
        queryClient.setQueryData<{ sessions: SessionSummary[] }>(['sessions'], (current) =>
          current
            ? {
                ...current,
                sessions: current.sessions.filter((session) => !deleted.has(session.id)),
              }
            : current,
        )
      }),
    [queryClient],
  )
  const sessions = useMemo(
    () => orderVisibleSessions(sidebarSessionData?.sessions || []),
    [sidebarSessionData],
  )
  const visibleSessions = useMemo(() => {
    const needle = sessionQuery.trim().toLocaleLowerCase(language)
    if (!needle) return sessions.filter((session) => !session.archived)
    return sessions.filter((session) =>
      `${session.name || ''} ${session.cwd || ''} ${workspaceNames[sessionWorkspaceKey(session)] || ''}`
        .toLocaleLowerCase(language)
        .includes(needle),
    )
  }, [language, sessionQuery, sessions, workspaceNames])
  const allSessionGroups = useMemo(() => groupSessionsByWorkspace(sessions), [sessions])
  useEffect(() => {
    rememberWorkspaces(allSessionGroups.map((group) => group.key))
  }, [allSessionGroups, rememberWorkspaces])
  const sessionGroups = useMemo(
    () =>
      orderWorkspaceGroups(
        recentWorkspaceGroups(visibleSessions, RECENT_SESSION_LIMIT, activeSessionId),
        workspaceOrder,
      ),
    [visibleSessions, workspaceOrder, activeSessionId],
  )
  const workspaceCounts = useMemo(
    () => new Map(allSessionGroups.map((group) => [group.key, group.sessions.length])),
    [allSessionGroups],
  )
  const menuTargetGroup = allSessionGroups.find((group) => group.key === menuTargetKey) || null
  const menuTargetSession = sessions.find((session) => session.id === menuTargetSessionId) || null
  const workspaceLabel = (group: SessionGroup) =>
    group.cwd
      ? workspaceNames[group.key] || workspaceName(group.cwd, language)
      : t('navigation:appSidebar.noWorkspace')

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
    setSessionQuery('')
    setHistoryExpanded(true)
    setCollapsedWorkspaces((current) => {
      const next = new Set(current)
      next.delete(sessionWorkspaceKey({ id: '', cwd }))
      return next
    })
    navigate('chat')
    if (isMobile) setOpenMobile(false)
  }

  // 删除途中失败也刷新目录并重新选择活动会话；只有 API 确认成功的 id 算入已删除数。
  const removeSessions = async (targets: SessionSummary[], baseline = sessions) => {
    const result = await deleteSessionsSequentially(
      targets.map((session) => session.id),
      (id) => apiJson(`/api/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' }),
    )
    announceSessionsUpdated(
      result.deletedIds.length ? { deletedIds: result.deletedIds } : undefined,
    )
    let refreshed: SessionSummary[] | null = null
    try {
      refreshed = (await fetchStartupQuery<{ sessions: SessionSummary[] }>('sessions', true))
        .sessions
    } catch {
      // 删除结果已被记录；目录暂时不可用时不把项目误报为完全删除。
    }
    const deletedIds = new Set(result.deletedIds)
    const remaining = refreshed ?? baseline.filter((session) => !deletedIds.has(session.id))
    const currentActiveId = localStorage.getItem(STORAGE_KEYS.activeSession) || activeSessionId
    const nextId = replacementActiveSessionId(
      currentActiveId,
      remaining,
      deletedIds,
      refreshed !== null,
    )
    if (nextId !== null) {
      setActiveSessionId(nextId)
      if (nextId) requestSessionSelection(nextId)
      else {
        localStorage.removeItem(STORAGE_KEYS.activeSession)
        announceActiveSession('')
      }
    }
    return { ...result, sessions: remaining, verified: refreshed !== null }
  }

  // 删除项目：重新读取完整目录，确认和执行都不依赖搜索结果与最近 24 条。
  const deleteProject = async (group: SessionGroup) => {
    if (deletingKey) return
    // 同名目录可能位于不同父目录；危险操作始终展示完整路径以明确范围。
    const label = group.cwd ? `${workspaceLabel(group)} · ${group.cwd}` : workspaceLabel(group)
    setDeletingKey(group.key)
    try {
      const freshSessions = (
        await fetchStartupQuery<{ sessions: SessionSummary[] }>('sessions', true)
      ).sessions
      const targets = sessionsInWorkspace(freshSessions, group.key)
      if (!targets.length) {
        notify(t('navigation:appSidebar.projectNoChats', { project: label }), 'info')
        return
      }
      const approved = await requestConfirm({
        title: t('navigation:appSidebar.deleteProject'),
        message: t('navigation:appSidebar.deleteProjectConfirm', {
          project: label,
          count: targets.length,
        }),
        confirmLabel: t('navigation:appSidebar.deleteProjectAction'),
      })
      if (!approved) return
      const result = await removeSessions(targets, freshSessions)
      if (!result.verified) {
        notify(
          t('navigation:appSidebar.projectDeleteUnverified', {
            project: label,
            deleted: result.deletedIds.length,
          }),
          'error',
        )
        return
      }
      const remainingCount = sessionsInWorkspace(result.sessions, group.key).length
      if (result.failedId !== null || remainingCount) {
        notify(
          t('navigation:appSidebar.projectDeletePartial', {
            project: label,
            deleted: result.deletedIds.length,
            remaining: remainingCount,
          }),
          'error',
        )
        return
      }
      notify(t('navigation:appSidebar.projectDeleted', { project: label }))
    } catch (error) {
      notify(
        error instanceof SessionOrganizationProtocolError
          ? t('chat:chatHistoryPage.sessionUpdateFailed')
          : error instanceof Error
            ? error.message
            : String(error),
        'error',
      )
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
      const result = await removeSessions([session])
      if (result.failedId !== null)
        throw result.error ?? new Error(t('navigation:appSidebar.chatDeleteFailed'))
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
      const updated = await apiJson<{ name: string }>(
        `/api/sessions/${encodeURIComponent(session.id)}`,
        {
          method: 'PATCH',
          body: JSON.stringify({ name }),
        },
      )
      announceSessionsUpdated({ id: session.id, name: updated.name })
      void fetchStartupQuery('sessions', true).catch(() => {})
      notify(t('chat:chatHistoryPage.chatTitleUpdated'))
    } catch (error) {
      notify(error instanceof Error ? error.message : String(error), 'error')
    }
  }

  const saveProjectName = (group: SessionGroup, name: string) => {
    try {
      setWorkspaceName(group.cwd, name)
      notify(t('navigation:appSidebar.projectNameUpdated'))
    } catch {
      notify(t('navigation:appSidebar.projectNameSaveFailed'), 'error')
    }
  }

  const renameProject = async (group: SessionGroup) => {
    if (!group.cwd) return
    const currentName = workspaceLabel(group)
    const name = await requestText({
      title: t('navigation:appSidebar.renameProject'),
      message: t('navigation:appSidebar.renameProjectDescription'),
      inputLabel: t('navigation:appSidebar.projectName'),
      value: currentName,
      maxLength: 120,
      confirmLabel: t('chat:chatHistoryPage.save'),
    })
    if (name === null || !name.trim() || name.trim() === currentName) return
    saveProjectName(group, name.trim())
  }

  const organizeSession = async (
    session: SessionSummary,
    patch: { pinned?: boolean; archived?: boolean; read?: boolean },
    message: string,
  ) => {
    if (deletingKey) return
    try {
      const updated = await updateSessionOrganization(session.id, patch)
      queryClient.setQueryData<{ sessions: SessionSummary[] }>(['sessions'], (current) =>
        current
          ? {
              ...current,
              sessions: current.sessions.map((item) =>
                item.id === updated.id ? { ...item, ...updated } : item,
              ),
            }
          : current,
      )
      notify(message)
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

  const setContextMenuTarget = (key = '', sessionId = '') => {
    setMenuTargetKey(key)
    setMenuTargetSessionId(sessionId)
  }

  // Radix 的触摸长按直接从 pointerdown 打开菜单，不会触发浏览器 contextmenu。
  const prepareTouchMenu = (event: PointerEvent, key = '', sessionId = '') => {
    if (event.pointerType !== 'mouse') setContextMenuTarget(key, sessionId)
  }

  const navigateFromSection = (id: string) => {
    navigate(id)
    if (isMobile) setOpenMobile(false)
  }

  return (
    <section
      className="flex min-h-0 flex-1 flex-col px-2 pt-3"
      aria-label={t('navigation:appSidebar.recentChats')}
    >
      <div className="mb-1 flex h-8 shrink-0 items-center gap-1 px-1.5">
        <button
          className="mr-auto flex min-w-0 items-center gap-1 rounded-md px-1 py-1 text-xs text-muted-foreground hover:text-foreground"
          aria-controls="sidebar-recent-sessions"
          aria-expanded={historyExpanded}
          onClick={() => setHistoryExpanded((value) => !value)}
        >
          <span>{t('navigation:workbench.projects')}</span>
          <ChevronRight className={historyExpanded ? 'rotate-90' : ''} size={12} />
        </button>
        <Button
          variant="ghost"
          size="icon-sm"
          className="size-6 text-muted-foreground"
          title={t('navigation:workbench.filterSessions')}
          aria-label={t('navigation:workbench.filterSessions')}
          aria-expanded={filterOpen}
          onClick={() => setFilterOpen((value) => !value)}
        >
          <Search size={13} />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className="size-6 text-muted-foreground"
          title={t('navigation:appSidebar.newProject')}
          aria-label={t('navigation:appSidebar.newProject')}
          aria-haspopup="dialog"
          aria-expanded={projectPickerOpen}
          onClick={() => setProjectPickerOpen(true)}
        >
          <FolderPlus size={14} />
        </Button>
        <button
          className="rounded-md px-1 py-1 text-[11px] text-muted-foreground hover:text-foreground"
          aria-label={t('navigation:appSidebar.viewAllCountChats', { count: sessions.length })}
          onClick={() => navigateFromSection('chatHistory')}
        >
          {t('navigation:appSidebar.viewAll')}
        </button>
      </div>
      {(filterOpen || sessionQuery) && (
        <label className="mx-2 mb-2 flex h-8 shrink-0 items-center gap-2 rounded-lg border border-border px-2 text-muted-foreground focus-within:ring-1 focus-within:ring-ring">
          <Search size={13} aria-hidden="true" />
          <input
            className="min-w-0 flex-1 border-0 bg-transparent text-xs outline-none"
            value={sessionQuery}
            onChange={(event) => setSessionQuery(event.target.value)}
            placeholder={t('navigation:appSidebar.searchChats')}
            aria-label={t('navigation:appSidebar.searchChats')}
          />
        </label>
      )}
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
              className="flex flex-1 min-h-0 flex-col gap-[2px] [padding-bottom:2px] overflow-y-auto [animation:page-in_var(--d1)_var(--ease-out)]"
              id="sidebar-recent-sessions"
              onPointerDownCapture={(event) => prepareTouchMenu(event)}
              onContextMenuCapture={() => setContextMenuTarget()}
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
                      onPointerDown={(event) => prepareTouchMenu(event, group.key)}
                    >
                      <button
                        className="flex h-8 min-w-0 flex-1 items-center gap-1.5 rounded-lg px-2 text-left text-[13px] text-foreground hover:bg-sidebar-accent [&_span]:min-w-0 [&_span]:flex-1 [&_span]:truncate [&_small]:hidden [&_.is-open]:rotate-90"
                        aria-expanded={!groupCollapsed}
                        onClick={() => toggleWorkspace(group.key)}
                        title={group.cwd || label}
                      >
                        <ChevronRight className={groupCollapsed ? '' : 'is-open'} size={13} />
                        <FolderClosed size={13} />
                        <span>{label}</span>
                        <small>{workspaceCounts.get(group.key) || group.sessions.length}</small>
                      </button>
                      {group.cwd && (
                        <>
                          <WorkspaceActionButton
                            label={t('navigation:appSidebar.renameProjectNamed', {
                              project: label,
                            })}
                            onClick={() => void renameProject(group)}
                          >
                            <Pencil size={14} aria-hidden="true" />
                          </WorkspaceActionButton>
                          <WorkspaceActionButton
                            label={t('navigation:appSidebar.newChatInWorkspace', {
                              workspace: group.cwd,
                            })}
                            onClick={() => createSessionInWorkspace(group.cwd)}
                          >
                            <Plus size={14} />
                          </WorkspaceActionButton>
                        </>
                      )}
                    </div>
                    {!groupCollapsed &&
                      group.sessions.map((session) => (
                        <button
                          className={`flex h-8 w-full items-center gap-1 rounded-lg pl-8 pr-2.5 text-left text-[13px] font-normal text-muted-foreground hover:bg-sidebar-accent hover:text-foreground ${session.id === activeSessionId ? 'active-session bg-sidebar-accent text-foreground' : ''}`}
                          aria-current={session.id === activeSessionId ? 'page' : undefined}
                          title={`${session.name || t('navigation:appSidebar.untitledChat')} · ${relativeTime(session.modified, language)}`}
                          onClick={() => openRecentSession(session.id)}
                          onContextMenu={() => setMenuTargetSessionId(session.id)}
                          onPointerDown={(event) => prepareTouchMenu(event, '', session.id)}
                          key={session.id}
                        >
                          {session.pinned && (
                            <Pin
                              className="mr-1 size-3 shrink-0"
                              aria-label={t('navigation:appSidebar.pinned')}
                            />
                          )}
                          {session.needsAttention && (
                            <CircleAlert
                              className="mr-1 size-3 shrink-0"
                              aria-label={t('navigation:appSidebar.needsAttention')}
                            />
                          )}
                          {session.unread && (
                            <span
                              className="mr-1 size-1.5 shrink-0 rounded-full bg-[var(--brand-blue)]"
                              aria-label={t('navigation:appSidebar.unread')}
                            />
                          )}
                          <span className="min-w-0 flex-1 select-none truncate">
                            {session.name || t('navigation:appSidebar.untitledChat')}
                          </span>
                          <time
                            className="shrink-0 text-[10px] text-muted-foreground/70"
                            dateTime={session.modified}
                          >
                            {relativeTime(session.modified, language)}
                          </time>
                          {session.archived && (
                            <span className="ml-auto text-[11px] font-normal text-muted-foreground">
                              {t('navigation:appSidebar.archived')}
                            </span>
                          )}
                        </button>
                      ))}
                  </div>
                )
              })}
              {!visibleSessions.length && (
                <span className="[padding:8px] text-[var(--text-muted)] text-[12px] font-normal">
                  {sessionQuery.trim()
                    ? t('navigation:appSidebar.noMatchingChats')
                    : sessions.length
                      ? t('navigation:appSidebar.noActiveChats')
                      : t('navigation:appSidebar.noChatHistoryYet')}
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
                  onSelect={() =>
                    void organizeSession(
                      menuTargetSession,
                      { pinned: !menuTargetSession.pinned },
                      menuTargetSession.pinned
                        ? t('navigation:appSidebar.unpinned')
                        : t('navigation:appSidebar.pinned'),
                    )
                  }
                >
                  {menuTargetSession.pinned ? <PinOff size={13} /> : <Pin size={13} />}
                  {menuTargetSession.pinned
                    ? t('navigation:appSidebar.unpin')
                    : t('navigation:appSidebar.pin')}
                </ContextMenuItem>
                <ContextMenuItem
                  onSelect={() =>
                    void organizeSession(
                      menuTargetSession,
                      { archived: !menuTargetSession.archived },
                      menuTargetSession.archived
                        ? t('navigation:appSidebar.restored')
                        : t('navigation:appSidebar.movedToArchive'),
                    )
                  }
                >
                  {menuTargetSession.archived ? (
                    <ArchiveRestore size={13} />
                  ) : (
                    <Archive size={13} />
                  )}
                  {menuTargetSession.archived
                    ? t('navigation:appSidebar.restore')
                    : t('navigation:appSidebar.archive')}
                </ContextMenuItem>
                {menuTargetSession.unread && (
                  <ContextMenuItem
                    onSelect={() =>
                      void organizeSession(
                        menuTargetSession,
                        { read: true },
                        t('navigation:appSidebar.markedRead'),
                      )
                    }
                  >
                    {t('navigation:appSidebar.markRead')}
                  </ContextMenuItem>
                )}
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
                  <>
                    <ContextMenuItem onSelect={() => createSessionInWorkspace(menuTargetGroup.cwd)}>
                      <Plus size={13} />
                      {t('navigation:appSidebar.newChat')}
                    </ContextMenuItem>
                    <ContextMenuItem onSelect={() => void renameProject(menuTargetGroup)}>
                      <Pencil size={13} />
                      {t('navigation:appSidebar.renameProject')}
                    </ContextMenuItem>
                    {workspaceNames[menuTargetGroup.key] && (
                      <ContextMenuItem onSelect={() => saveProjectName(menuTargetGroup, '')}>
                        <RotateCcw size={13} />
                        {t('navigation:appSidebar.resetProjectName')}
                      </ContextMenuItem>
                    )}
                  </>
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
                  <ContextMenuSubTrigger
                    disabled={!allSessionGroups.length || Boolean(deletingKey)}
                  >
                    <Trash2 size={13} />
                    {t('navigation:appSidebar.deleteProject')}
                  </ContextMenuSubTrigger>
                  <ContextMenuSubContent className="max-h-[70vh] max-w-[280px] overflow-y-auto">
                    {allSessionGroups.map((group) => (
                      <ContextMenuItem key={group.key} onSelect={() => void deleteProject(group)}>
                        <span className="truncate" title={group.cwd || workspaceLabel(group)}>
                          {workspaceLabel(group)}
                        </span>
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
