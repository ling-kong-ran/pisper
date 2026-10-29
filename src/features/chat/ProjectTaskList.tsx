// 侧边栏「最近会话」区块：搜索、按工作区分组的会话列表与右键菜单
//（空白处新建项目/删除项目、分组上新建会话/重命名项目/删除项目）。
// 由应用壳经公共导航入口懒加载，任务组织规则由聊天领域持有。
// 目录选择弹窗与 radix 右键菜单原语只在区块内按需加载。
import {
  lazy,
  Suspense,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent,
  type ReactNode,
} from 'react'
import { useQueryClient } from '@tanstack/react-query'
import {
  Archive,
  ArchiveRestore,
  ChevronRight,
  FolderClosed,
  FolderPlus,
  MessageSquare,
  PanelRight,
  Pencil,
  Pin,
  PinOff,
  Plus,
  RotateCcw,
  Search,
  Trash2,
} from 'lucide-react'
import type { SessionSplitDisposition } from './dock-layout'
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
import { BOARD_SESSIONS_QUERY_KEY, useSessionListQuery } from '@/features/board/public'
import { fetchStartupQuery } from '@/lib/startup-queries'
import { pageStateStorage } from '@/lib/page-state-storage'
import { apiJson } from '@/lib/api'
import { relativeTime, workspaceName } from '@/lib/format'
import { pickSystemDirectory } from '@/lib/pick-system-directory'
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
import { OrbitSpinner } from '@/components/ui/spinner'
import { StarOrbit } from '@/components/StarOrbit'
import { cn } from '@/lib/utils'

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
  attentionReason?: 'approval' | 'failure' | null
  streaming?: boolean
}

type SessionGroup = WorkspaceSessionGroup<SessionSummary>

type ProjectTaskListProps = {
  navigate: (page: string) => void
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  notify: Notify
}

const RECENT_SESSION_LIMIT = 24

// 树形行的共用几何：默认 36px、紧凑密度 30px，手机与粗指针放大到 44px 触控目标。
const TREE_ROW =
  'h-9 rounded-sm text-sm transition-colors duration-(--d-fast) ease-precise in-data-[density=compact]:h-[30px] max-[650px]:h-11 pointer-coarse:h-11'

// 分组头部的小图标键：默认 24px，手机与粗指针放大到 44px。
const SECTION_ICON_BUTTON = 'size-6 text-content-muted max-[650px]:size-11 pointer-coarse:size-11'

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
      className="size-7 shrink-0 p-0 text-content-muted hover:bg-surface-active hover:text-content max-[650px]:size-11 pointer-coarse:size-11"
      title={label}
      aria-label={label}
      onClick={onClick}
    >
      {children}
    </Button>
  )
}

// 会话状态只占一个固定宽度的槽位，保证标题左缘对齐：运行用品牌轨道指示，
// 失败为危险色点、待审批/需处理为警示色点、未读为墨色点；颜色之外都带可读标签。
function SessionStatusMark({
  session,
  labels,
}: {
  session: SessionSummary
  labels: { running: string; failed: string; approval: string; attention: string; unread: string }
}) {
  // 运行指示作为带名图片参与按钮的可访问名称，保持「正在执行 <标题> <时间>」的朗读顺序。
  if (session.streaming)
    return (
      <OrbitSpinner
        role="img"
        aria-hidden={false}
        aria-label={labels.running}
        className="size-3.5"
      />
    )
  const dot = (label: string, tone: string) => (
    <span role="img" aria-label={label} className={cn('size-1.5 rounded-full', tone)} />
  )
  if (session.needsAttention) {
    if (session.attentionReason === 'failure') return dot(labels.failed, 'bg-danger')
    return dot(
      session.attentionReason === 'approval' ? labels.approval : labels.attention,
      'bg-warning-strong',
    )
  }
  if (session.unread) return dot(labels.unread, 'bg-ink')
  return null
}

export function ProjectTaskList({
  navigate,
  requestText,
  requestConfirm,
  notify,
}: ProjectTaskListProps) {
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
    () => pageStateStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  // 右键菜单状态：menuTargetKey 记录右键落在哪个工作区分组（空串=空白区域），
  // menuTargetSessionId 记录右键落点是否为某个会话（优先级高于分组）；
  // projectPickerOpen 控制新建项目的目录选择弹窗，deletingKey 防止重复提交删除。
  const [menuTargetKey, setMenuTargetKey] = useState('')
  const [menuTargetSessionId, setMenuTargetSessionId] = useState('')
  const [projectPickerOpen, setProjectPickerOpen] = useState(false)
  const [projectPicking, setProjectPicking] = useState(false)
  const projectPickingRef = useRef(false)
  const projectRequestRef = useRef(0)
  const [deletingKey, setDeletingKey] = useState('')

  useEffect(
    () => () => {
      projectRequestRef.current += 1
    },
    [],
  )

  // 侧栏只读看板持有的权威会话目录；轮询（可见时 3 秒、隐藏时停止）由看板同步统一负责。
  const { data: sidebarSessionData } = useSessionListQuery<{ sessions: SessionSummary[] }>()
  useEffect(
    () =>
      subscribeSessionDeletionUpdates(window, ({ deletedIds }) => {
        const deleted = new Set(deletedIds)
        queryClient.setQueryData<{ sessions: SessionSummary[] }>(
          BOARD_SESSIONS_QUERY_KEY,
          (current) =>
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

  // 回到前台后的刷新由看板同步负责，这里只同步活动会话高亮。
  useEffect(() => {
    const syncActive = (event: Event) => {
      const detail = (event as CustomEvent<{ id?: string }>).detail
      setActiveSessionId(detail?.id || pageStateStorage.getItem(STORAGE_KEYS.activeSession) || '')
    }
    window.addEventListener(SESSION_SELECTED_EVENT, syncActive)
    window.addEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
    return () => {
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

  const splitSession = (id: string, direction: SessionSplitDisposition) => {
    requestSessionSelection(id, direction)
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

  const chooseProject = async () => {
    if (projectPickingRef.current) return
    projectPickingRef.current = true
    setProjectPicking(true)
    const request = ++projectRequestRef.current
    try {
      const result = await pickSystemDirectory('', t('common:workspacePicker.setWorkingDirectory'))
      if (request !== projectRequestRef.current) return
      if (result.status === 'picked') createSessionInWorkspace(result.path)
      // 系统对话框无法出现时（配对设备、无图形会话）才退回手动填写路径。
      else if (result.status === 'unavailable') setProjectPickerOpen(true)
    } catch (error) {
      if (request === projectRequestRef.current)
        notify(error instanceof Error ? error.message : String(error), 'error')
    } finally {
      // 原生对话框可能晚于侧栏卸载才返回，旧结果不能创建会话或更新已卸载状态。
      if (request === projectRequestRef.current) {
        projectPickingRef.current = false
        setProjectPicking(false)
      }
    }
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
    const currentActiveId = pageStateStorage.getItem(STORAGE_KEYS.activeSession) || activeSessionId
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
        pageStateStorage.removeItem(STORAGE_KEYS.activeSession)
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
      queryClient.setQueryData<{ sessions: SessionSummary[] }>(
        BOARD_SESSIONS_QUERY_KEY,
        (current) =>
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

  const statusLabels = {
    running: t('navigation:appSidebar.running'),
    failed: t('navigation:appSidebar.failed'),
    approval: t('navigation:appSidebar.awaitingApproval'),
    attention: t('navigation:appSidebar.needsAttention'),
    unread: t('navigation:appSidebar.unread'),
  }

  return (
    <section
      className="flex min-h-0 flex-1 flex-col px-1 pt-4"
      aria-label={t('navigation:appSidebar.recentChats')}
    >
      {/* 分组标签：12/500 弱化文字，右侧是筛选、新建项目与查看全部，几何与侧栏其它分组一致。 */}
      <div className="flex h-7 shrink-0 items-center gap-0.5 pr-1.5 pl-2 max-[650px]:h-11">
        <button
          className="mr-auto flex h-6 min-w-0 items-center gap-1 rounded-sm px-1.5 text-xs font-medium tracking-[.02em] text-content-muted transition-colors duration-(--d-fast) hover:text-content max-[650px]:h-11 pointer-coarse:h-11"
          aria-controls="sidebar-recent-sessions"
          aria-expanded={historyExpanded}
          onClick={() => setHistoryExpanded((value) => !value)}
        >
          <span>{t('navigation:workbench.projects')}</span>
          <ChevronRight
            className={cn(
              'transition-transform duration-(--d-fast) ease-precise',
              historyExpanded && 'rotate-90',
            )}
            size={12}
          />
        </button>
        <Button
          variant="ghost"
          size="icon-sm"
          className={SECTION_ICON_BUTTON}
          title={t('navigation:workbench.filterSessions')}
          aria-label={t('navigation:workbench.filterSessions')}
          aria-expanded={filterOpen}
          onClick={() => setFilterOpen((value) => !value)}
        >
          <Search className="size-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className={SECTION_ICON_BUTTON}
          title={t('navigation:appSidebar.newProject')}
          aria-label={t('navigation:appSidebar.newProject')}
          aria-haspopup="dialog"
          aria-expanded={projectPickerOpen}
          disabled={projectPicking}
          onClick={() => void chooseProject()}
        >
          <FolderPlus className="size-3.5" />
        </Button>
        <button
          className="h-6 rounded-sm px-1.5 text-xs text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content max-[650px]:h-11 pointer-coarse:h-11"
          aria-label={t('navigation:appSidebar.viewAllCountChats', { count: sessions.length })}
          onClick={() => navigateFromSection('chatHistory')}
        >
          {t('navigation:appSidebar.viewAll')}
        </button>
      </div>
      {(filterOpen || sessionQuery) && (
        <label className="m-1 flex h-8 shrink-0 items-center gap-2 rounded-sm border border-control bg-panel px-2.5 text-content-muted focus-within:border-focus focus-within:ring-3 focus-within:ring-focus-ring max-[650px]:h-11">
          <Search size={14} aria-hidden="true" />
          <input
            className="min-w-0 flex-1 border-0 bg-transparent text-sm text-content caret-focus outline-none max-[650px]:text-[16px]"
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
              className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto p-1 pb-2 [animation:page-in_var(--d1)_var(--ease-out)]"
              id="sidebar-recent-sessions"
              onPointerDownCapture={(event) => prepareTouchMenu(event)}
              onContextMenuCapture={() => setContextMenuTarget()}
            >
              {sessionGroups.map((group) => {
                const groupCollapsed = collapsedWorkspaces.has(group.key)
                const label = workspaceLabel(group)
                return (
                  <div className="nav-workspace-group" key={group.key}>
                    {/* 项目行：操作键在悬停或键盘聚焦时替换右侧计数，粗指针下常驻。 */}
                    <div
                      className="group/workspace flex min-w-0 items-center gap-0.5 rounded-sm pr-1 hover:bg-surface-hover"
                      onContextMenu={() => setMenuTargetKey(group.key)}
                      onPointerDown={(event) => prepareTouchMenu(event, group.key)}
                    >
                      <button
                        className={cn(
                          TREE_ROW,
                          'flex min-w-0 flex-1 items-center gap-1.5 px-1.5 text-left font-medium text-content',
                        )}
                        aria-expanded={!groupCollapsed}
                        onClick={() => toggleWorkspace(group.key)}
                        title={group.cwd || label}
                      >
                        <ChevronRight
                          className={cn(
                            'shrink-0 text-content-muted transition-transform duration-(--d-fast) ease-precise',
                            !groupCollapsed && 'rotate-90',
                          )}
                          size={14}
                        />
                        <FolderClosed className="shrink-0 text-content-secondary" size={16} />
                        <span className="min-w-0 flex-1 truncate">{label}</span>
                        <small
                          className={cn(
                            'shrink-0 text-xs font-normal text-content-muted tabular-nums',
                            group.cwd &&
                              'group-focus-within/workspace:hidden group-hover/workspace:hidden pointer-coarse:hidden',
                          )}
                        >
                          {workspaceCounts.get(group.key) || group.sessions.length}
                        </small>
                      </button>
                      {group.cwd && (
                        <span className="hidden shrink-0 items-center gap-0.5 group-focus-within/workspace:flex group-hover/workspace:flex pointer-coarse:flex">
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
                        </span>
                      )}
                    </div>
                    {/* 会话缩进挂在 1px 引导线上；当前会话抬起为键帽，3px 强调刻度压在引导线上。 */}
                    {!groupCollapsed && group.sessions.length > 0 && (
                      <div className="relative flex flex-col gap-px pt-px pl-6 before:absolute before:top-0.5 before:bottom-0.5 before:left-[13px] before:w-px before:bg-stroke-strong">
                        {group.sessions.map((session) => {
                          const active = session.id === activeSessionId
                          return (
                            <button
                              className={cn(
                                TREE_ROW,
                                'relative flex w-full min-w-0 items-center gap-2 pr-2 pl-2.5 text-left text-content-secondary hover:bg-surface-hover hover:text-content',
                                active &&
                                  'active-session bg-key font-medium text-content shadow-key ring-1 ring-key-border before:absolute before:top-1/2 before:-left-3 before:h-4 before:w-[3px] before:-translate-y-1/2 before:rounded-r-[3px] before:bg-star hover:bg-key',
                              )}
                              aria-current={active ? 'page' : undefined}
                              title={`${session.name || t('navigation:appSidebar.untitledChat')} · ${relativeTime(session.modified, language)}`}
                              onClick={() => openRecentSession(session.id)}
                              onContextMenu={() => setMenuTargetSessionId(session.id)}
                              onPointerDown={(event) => prepareTouchMenu(event, '', session.id)}
                              key={session.id}
                            >
                              <span className="grid size-3.5 shrink-0 place-items-center">
                                <SessionStatusMark session={session} labels={statusLabels} />
                              </span>
                              <span className="min-w-0 flex-1 truncate select-none">
                                {session.name || t('navigation:appSidebar.untitledChat')}
                              </span>
                              {session.pinned && (
                                <Pin
                                  className="size-3 shrink-0 text-content-muted"
                                  aria-label={t('navigation:appSidebar.pinned')}
                                />
                              )}
                              {/* 运行中直接写出状态文字，不只依赖轨道指示的颜色与动画；
                                  读屏已从轨道指示读到「正在执行」，这里只给视觉，时间改为仅读屏可见。 */}
                              {session.streaming && (
                                <span
                                  aria-hidden="true"
                                  className="shrink-0 text-xs font-medium text-accent-fg"
                                >
                                  {statusLabels.running}
                                </span>
                              )}
                              <time
                                className={cn(
                                  'shrink-0 text-xs font-normal text-content-muted tabular-nums',
                                  session.streaming && 'sr-only',
                                )}
                                dateTime={session.modified}
                              >
                                {relativeTime(session.modified, language)}
                              </time>
                              {session.archived && (
                                <span className="shrink-0 text-xs font-normal text-content-muted">
                                  {t('navigation:appSidebar.archived')}
                                </span>
                              )}
                            </button>
                          )
                        })}
                      </div>
                    )}
                  </div>
                )
              })}
              {!visibleSessions.length && (
                <div className="mt-1 flex items-center gap-3 rounded-lg border border-dashed border-stroke-strong px-3 py-3 text-xs leading-relaxed text-content-muted">
                  <StarOrbit size={27} />
                  <span>
                    {sessionQuery.trim()
                      ? t('navigation:appSidebar.noMatchingChats')
                      : sessions.length
                        ? t('navigation:appSidebar.noActiveChats')
                        : t('navigation:appSidebar.noChatHistoryYet')}
                  </span>
                </div>
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
                {!isMobile && (
                  <ContextMenuSub>
                    <ContextMenuSubTrigger>
                      <PanelRight size={13} />
                      {t('chat:workbench.openSplit')}
                    </ContextMenuSubTrigger>
                    <ContextMenuSubContent>
                      <ContextMenuItem onSelect={() => splitSession(menuTargetSession.id, 'left')}>
                        {t('chat:focusSession.splitToLeft')}
                      </ContextMenuItem>
                      <ContextMenuItem onSelect={() => splitSession(menuTargetSession.id, 'right')}>
                        {t('chat:focusSession.splitToRight')}
                      </ContextMenuItem>
                      <ContextMenuItem onSelect={() => splitSession(menuTargetSession.id, 'above')}>
                        {t('chat:focusSession.splitToTop')}
                      </ContextMenuItem>
                      <ContextMenuItem onSelect={() => splitSession(menuTargetSession.id, 'below')}>
                        {t('chat:focusSession.splitToBottom')}
                      </ContextMenuItem>
                    </ContextMenuSubContent>
                  </ContextMenuSub>
                )}
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
                <ContextMenuItem disabled={projectPicking} onSelect={() => void chooseProject()}>
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
