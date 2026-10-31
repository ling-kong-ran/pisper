// 历史会话页（功能 › 会话与资料）：搜索完整会话目录并分批展示，支持继续打开和管理。
// 用功能页模板（规格 v2 第 10 节 A6 第 2 批）：“‹ 功能”· 标题 · 工具行（搜索、新任务、未归档 / 已归档）·
// 按置顶 / 今天 / 近 7 天 / 更早分区的列表行（名称、首条消息、工作区与消息数、右侧时间与 ⋯ 菜单）。
// 会话目录只有一个权威所有者（['sessions','board'] 缓存，由看板同步每 3 秒刷新）：本页只读这份缓存，
// 写操作把服务端确认的结果写回缓存，不再保留第二份本地列表，也不自带轮询。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  Archive,
  ArchiveRestore,
  FolderClosed,
  MoreHorizontal,
  Pencil,
  Pin,
  PinOff,
  RefreshCw,
  SquarePen,
  Trash2,
} from 'lucide-react'
import { APP_NAME } from '@/app/brand'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { STORAGE_KEYS } from '@/app/storage'
import { FEATURE_SEARCH_INPUT_ID } from '@/app/more-pages'
import { pageStateStorage } from '@/lib/page-state-storage'
import {
  FeatureList,
  FeaturePage,
  FeatureRow,
  FeatureSearch,
  FeatureSection,
  FeatureToolbar,
} from '@/components/app/FeaturePage'
import { PageState } from '@/components/app/PageState'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { apiJson } from '@/lib/api'
import { relativeTime, workspaceName } from '@/lib/format'
import { useShortcutLabel } from '@/lib/shortcuts'
import {
  fetchStartupQuery,
  queryClient,
  SESSIONS_QUERY_KEY,
  startupQueryOptions,
} from '@/lib/startup-queries'
import type { SessionSummary } from '@/types/chat'
import {
  ACTIVE_SESSION_CHANGED_EVENT,
  announceActiveSession,
  announceSessionsUpdated,
  requestSessionSelection,
} from './events'
import {
  HISTORY_BATCH_SIZE,
  selectHistorySessions,
  type HistoryGroupKey,
  type HistoryView,
} from './history-list'
import { applySessionOrganizationUpdate, orderVisibleSessions } from './session-list'
import {
  SessionOrganizationProtocolError,
  updateSessionOrganization,
  type SessionOrganizationPatch,
} from './session-organization-api'
import { SessionChangeIndicator } from './SessionChangeIndicator'

type SessionsData = { sessions?: SessionSummary[] }

// 写回权威缓存：与看板、聊天目录读到的是同一份列表，下一轮轮询以服务端快照为准。
function writeSessions(update: (sessions: SessionSummary[]) => SessionSummary[]) {
  queryClient.setQueryData<SessionsData>(SESSIONS_QUERY_KEY, (current) =>
    current ? { ...current, sessions: update(current.sessions || []) } : current,
  )
}

type ChatHistoryPageProps = {
  navigate: (page: string) => void
  // 从“功能”进入时由应用壳提供，渲染标题上方的“‹ 功能”。
  onBack?: () => void
  // 工具行主操作“新任务”：打开派发对话框（与侧栏、⌘N 同一入口）。
  onNewTask?: () => void
  notify: Notify
  requestConfirm: (options: ConfirmDialogOptions) => Promise<boolean>
  requestText: (options: PromptDialogOptions) => Promise<string | null>
}

export function ChatHistoryPage({
  navigate,
  onBack,
  onNewTask,
  notify,
  requestConfirm,
  requestText,
}: ChatHistoryPageProps) {
  const { t, language } = useI18n()
  const searchShortcut = useShortcutLabel('focusSearch')
  // 搜索词归页面自己（换页即清空，与原共享页头一致）；外壳的“/”按固定 id 聚焦这里的搜索框。
  const [query, setQuery] = useState('')
  const sessionsQuery = useQuery(startupQueryOptions<SessionsData>('sessions'))
  const sessions = useMemo(
    () => orderVisibleSessions(sessionsQuery.data?.sessions || []),
    [sessionsQuery.data],
  )
  const [historyView, setHistoryView] = useState<HistoryView>('active')
  const filterKey = `${historyView}\0${query}`
  const [pageLimit, setPageLimit] = useState({ filterKey, count: HISTORY_BATCH_SIZE })
  const completedBatchFocusRef = useRef(false)
  const completedBatchStatusRef = useRef<HTMLParagraphElement>(null)
  const [activeId, setActiveId] = useState(
    () => pageStateStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState('')
  const loading = sessionsQuery.isPending || refreshing
  const listError =
    sessionsQuery.error && !sessionsQuery.data
      ? sessionsQuery.error instanceof Error
        ? sessionsQuery.error.message
        : String(sessionsQuery.error)
      : ''

  useEffect(() => {
    setPageLimit((current) =>
      current.filterKey === filterKey && current.count === HISTORY_BATCH_SIZE
        ? current
        : { filterKey, count: HISTORY_BATCH_SIZE },
    )
  }, [filterKey])

  // 手动刷新：与轮询共用同一个在途请求和同一份缓存。
  const load = useCallback(async () => {
    setRefreshing(true)
    setError('')
    try {
      await fetchStartupQuery<SessionsData>('sessions', true)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setRefreshing(false)
    }
  }, [])

  useEffect(() => {
    document.title = `${t('chat:chatHistoryPage.chatHistory')} · ${APP_NAME}`
    return () => {
      document.title = APP_NAME
    }
  }, [t])

  useEffect(() => {
    const syncActive = (event: Event) =>
      setActiveId(
        (event as CustomEvent<{ id?: string }>).detail?.id ||
          pageStateStorage.getItem(STORAGE_KEYS.activeSession) ||
          '',
      )
    window.addEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
    return () => window.removeEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
  }, [])

  // 搜索先覆盖完整目录，再限制挂载行数；切换搜索词立即回到首批结果。
  const visibleCount = pageLimit.filterKey === filterKey ? pageLimit.count : HISTORY_BATCH_SIZE
  const visible = useMemo(
    () => selectHistorySessions(sessions, query, visibleCount, historyView),
    [historyView, query, sessions, visibleCount],
  )
  const archivedCount = sessions.filter((session) => session.archived).length
  const viewCounts: Record<HistoryView, number> = {
    active: sessions.length - archivedCount,
    archived: archivedCount,
  }
  const groupLabels: Record<HistoryGroupKey, string> = {
    pinned: t('chat:chatHistoryPage.groupPinned'),
    today: t('chat:chatHistoryPage.groupToday'),
    week: t('chat:chatHistoryPage.groupWeek'),
    earlier: t('chat:chatHistoryPage.groupEarlier'),
  }

  useEffect(() => {
    if (!completedBatchFocusRef.current) return
    completedBatchFocusRef.current = false
    completedBatchStatusRef.current?.focus()
  }, [visible.items.length, visible.total])

  const openSession = (id: string) => {
    requestSessionSelection(id)
    navigate('chat')
  }

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
      writeSessions((current) =>
        current.map((item) => (item.id === session.id ? { ...item, name: updated.name } : item)),
      )
      announceSessionsUpdated({ id: session.id, name: updated.name })
      notify(t('chat:chatHistoryPage.chatTitleUpdated'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  const organizeSession = async (
    session: SessionSummary,
    patch: SessionOrganizationPatch,
    message: string,
  ) => {
    try {
      const updated = await updateSessionOrganization(session.id, patch)
      writeSessions((current) => applySessionOrganizationUpdate(current, updated))
      notify(message)
    } catch (caught) {
      setError(
        caught instanceof SessionOrganizationProtocolError
          ? t('chat:chatHistoryPage.sessionUpdateFailed')
          : caught instanceof Error
            ? caught.message
            : String(caught),
      )
    }
  }

  const deleteSession = async (session: SessionSummary) => {
    const approved = await requestConfirm({
      title: t('chat:chatHistoryPage.deleteChat'),
      message: t('chat:chatHistoryPage.deleteChatNameThisAlsoRemovesLocalChatHistory', {
        name: session.name,
      }),
      confirmLabel: t('chat:chatHistoryPage.delete'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/sessions/${encodeURIComponent(session.id)}`, { method: 'DELETE' })
      const remaining = sessions.filter((item) => item.id !== session.id)
      writeSessions((current) => current.filter((item) => item.id !== session.id))
      if (activeId === session.id) {
        const nextId = remaining[0]?.id || ''
        setActiveId(nextId)
        if (nextId) pageStateStorage.setItem(STORAGE_KEYS.activeSession, nextId)
        else pageStateStorage.removeItem(STORAGE_KEYS.activeSession)
        announceActiveSession(nextId)
      }
      announceSessionsUpdated({ deletedIds: [session.id] })
      notify(t('chat:chatHistoryPage.chatDeleted'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  // 状态只反映目录里已有的运行、失败、待审批与需处理字段，不推测其它状态。
  // 规格 6.1 / 6.5：运行中是中性弧；失败、待审批、需要处理都在等人，用琥珀圆点 + 原因文字。
  const statusLabel = (session: SessionSummary) => {
    if (session.streaming)
      return (
        <span className="inline-flex shrink-0 items-center gap-1 text-content-secondary">
          <StatusGlyph status="run" className="size-3.5" />
          {t('chat:chatHistoryPage.statusRunning')}
        </span>
      )
    if (!session.needsAttention) return null
    return (
      <span className="inline-flex shrink-0 items-center gap-1 font-medium text-warning-strong">
        <StatusGlyph status="attention" className="size-3.5" />
        {session.attentionReason === 'failure'
          ? t('chat:chatHistoryPage.statusFailed')
          : session.attentionReason === 'approval'
            ? t('chat:chatHistoryPage.statusApproval')
            : t('chat:chatHistoryPage.needsAttention')}
      </span>
    )
  }

  const title = t('chat:chatHistoryPage.chatHistory')
  const emptyTitle = query
    ? t('chat:chatHistoryPage.noMatchingConversations')
    : historyView === 'archived'
      ? t('chat:chatHistoryPage.noArchivedChats')
      : sessions.length
        ? t('chat:chatHistoryPage.noActiveChats')
        : t('chat:chatHistoryPage.noConversationsHaveLeftAnEchoYet')
  const emptyDescription = query
    ? t('chat:chatHistoryPage.tryADifferentSearch')
    : historyView === 'archived'
      ? t('chat:chatHistoryPage.archivedChatsAppearHere')
      : sessions.length
        ? t('chat:chatHistoryPage.restoreArchivedChats')
        : t('chat:chatHistoryPage.onceAConversationBeginsItWillBeQuietlyGatheredHere')

  return (
    <FeaturePage
      title={title}
      description={t('chat:chatHistoryPage.lede')}
      back={
        onBack && {
          label: t('navigation:sidebar.features'),
          ariaLabel: t('navigation:shell.backToFeaturesLabel'),
          onBack,
        }
      }
      toolbar={
        <FeatureToolbar
          search={
            <FeatureSearch
              id={FEATURE_SEARCH_INPUT_ID}
              value={query}
              onValueChange={setQuery}
              placeholder={t('navigation:pageHeader.searchPage', { page: title })}
              shortcut={searchShortcut}
              clearLabel={t('navigation:more.clearSearch')}
            />
          }
          actions={
            onNewTask && (
              // 主操作（原共享页头的“新会话”）：与侧栏“新任务”、⌘N 同一个派发对话框（规格 2.5）。
              <Button type="button" className="h-9" onClick={onNewTask}>
                <SquarePen />
                {t('navigation:shell.newTask')}
              </Button>
            )
          }
          filters={
            // 分段控件与统计同一行：选中段抬起为键帽，计数用等宽数字；手机上分段铺满整行、每段 44。
            <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
              <div
                role="group"
                aria-label={t('chat:chatHistoryPage.historySections')}
                className="inline-grid h-8 grid-cols-2 rounded-md border border-stroke bg-chrome p-[3px] max-[650px]:h-auto max-[650px]:flex-1 max-[650px]:rounded-[10px] pointer-coarse:h-auto"
              >
                {(['active', 'archived'] as const).map((view) => (
                  <button
                    key={view}
                    type="button"
                    aria-pressed={historyView === view}
                    aria-label={
                      view === 'active'
                        ? t('chat:chatHistoryPage.activeChatsCount', { count: viewCounts.active })
                        : t('chat:chatHistoryPage.archivedChats', { count: viewCounts.archived })
                    }
                    className={cn(
                      'inline-flex min-w-0 items-center justify-center gap-1.5 rounded-sm border border-transparent px-3 text-sm text-content-secondary transition-colors duration-(--d-fast) hover:text-content max-[650px]:h-11 max-[650px]:text-base pointer-coarse:h-11',
                      historyView === view &&
                        'border-key-border bg-key font-medium text-content shadow-key',
                    )}
                    onClick={() => setHistoryView(view)}
                  >
                    {view === 'active'
                      ? t('chat:chatHistoryPage.activeChats')
                      : t('chat:chatHistoryPage.archived')}
                    <span
                      className={cn(
                        'text-xs font-normal tabular-nums',
                        historyView === view ? 'text-content-secondary' : 'text-content-muted',
                      )}
                    >
                      {viewCounts[view]}
                    </span>
                  </button>
                ))}
              </div>
              <div className="flex min-w-0 items-center gap-1 text-xs text-content-muted tabular-nums">
                <span className={cn('truncate', !query && 'max-[650px]:hidden')}>
                  {t('chat:chatHistoryPage.countChatHistoryItems', { count: sessions.length })}
                  {' · '}
                  {query
                    ? t('chat:chatHistoryPage.countCurrentlyFiltered', { count: visible.total })
                    : t('chat:chatHistoryPage.sortedByMostRecentlyUpdated')}
                </span>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className="text-content-muted max-[650px]:size-11 pointer-coarse:size-11"
                  aria-label={t('chat:chatHistoryPage.refresh')}
                  title={t('chat:chatHistoryPage.refresh')}
                  onClick={() => void load()}
                  disabled={loading}
                >
                  {loading ? <OrbitSpinner className="text-current" /> : <RefreshCw size={14} />}
                </Button>
              </div>
            </div>
          }
        />
      }
    >
      {loading && !sessions.length ? (
        <PageState kind="loading" title={t('chat:chatHistoryPage.loadingChatHistory')} />
      ) : listError && !sessions.length ? (
        // 首次读取失败不显示成“还没有会话”：写明错误并给重试（规格 1.5 诚实降级）。
        <PageState
          kind="error"
          title={t('chat:chatHistoryPage.loadFailed')}
          description={listError}
          action={
            <Button
              variant="secondary"
              className="max-[650px]:h-11"
              loading={loading}
              disabled={loading}
              onClick={() => void load()}
            >
              {t('common:pageTemplate.retry')}
            </Button>
          }
        />
      ) : (
        <>
          {/* 读取成功后的操作失败（重命名、整理、删除）与刷新失败：写在列表上方，列表保留。 */}
          {(error || listError) && (
            <p
              role="alert"
              className="mt-4 rounded-[var(--r-md)] border border-danger-border bg-danger-soft px-3 py-2 text-xs [overflow-wrap:anywhere] text-danger"
            >
              {error || listError}
            </p>
          )}
          {visible.total ? (
            <div className="chat-history-list">
              {visible.groups.map((group) => (
                // 分组（置顶 / 今天 / 近 7 天 / 更早）各是一个分区，标题旁是该组在完整筛选结果中的会话数。
                <FeatureSection key={group.key} title={groupLabels[group.key]} count={group.count}>
                  <FeatureList>
                    {group.items.map((session) => {
                      const name = session.name || t('chat:chatHistoryPage.untitledChat')
                      const model =
                        session.model && !/(^|\/)unknown$/i.test(String(session.model))
                          ? String(session.model).split('/').at(-1)
                          : ''
                      const active = session.id === activeId
                      return (
                        <FeatureRow
                          key={session.id}
                          data-history-session={session.id}
                          data-current={active || undefined}
                          // 本设备当前会话：名称前 3px 石墨刻度（不用强调色，规格 6.1）。
                          className={cn(
                            'chat-history-row relative',
                            active &&
                              'after:absolute after:top-3.5 after:bottom-3.5 after:-left-3 after:w-[3px] after:rounded-r-[3px] after:bg-star max-[650px]:after:-left-2.5',
                          )}
                          onOpen={() => openSession(session.id)}
                          openLabel={t('chat:chatHistoryPage.openChatNamed', { name })}
                          name={
                            <span className="chat-history-copy inline-flex max-w-full min-w-0 items-center gap-2">
                              {session.unread && (
                                <span
                                  role="img"
                                  aria-label={t('chat:chatHistoryPage.unread')}
                                  className="size-1.5 shrink-0 rounded-full bg-ink"
                                />
                              )}
                              {session.pinned && (
                                <Pin
                                  className="size-3.5 shrink-0 text-content-muted"
                                  aria-hidden="true"
                                />
                              )}
                              <strong
                                data-history-name
                                className="min-w-0 truncate font-medium"
                                title={name}
                              >
                                {name}
                              </strong>
                            </span>
                          }
                          description={
                            <span
                              className={cn(
                                'line-clamp-1',
                                session.firstMessage ? 'text-content-secondary' : undefined,
                              )}
                            >
                              {session.firstMessage || t('chat:chatHistoryPage.noMessageSummary')}
                            </span>
                          }
                          meta={
                            <>
                              {statusLabel(session)}
                              {session.archived && query.trim() && (
                                <Badge>{t('chat:chatHistoryPage.archived')}</Badge>
                              )}
                              <span className="inline-flex min-w-0 items-center gap-1 font-mono">
                                <FolderClosed className="size-3.5 shrink-0" aria-hidden="true" />
                                <span className="truncate">
                                  {workspaceName(session.cwd, language)}
                                </span>
                              </span>
                              <span aria-hidden="true">·</span>
                              <span className="shrink-0 tabular-nums">
                                {t('chat:chatHistoryPage.countMessages', {
                                  count: session.messageCount || 0,
                                })}
                              </span>
                              {model && (
                                <>
                                  <span aria-hidden="true" className="max-[650px]:hidden">
                                    ·
                                  </span>
                                  <span className="min-w-0 truncate font-mono max-[650px]:hidden">
                                    {model}
                                  </span>
                                </>
                              )}
                              <SessionChangeIndicator
                                sessionId={session.id}
                                lastCompletedAt={session.lastCompletedAt}
                                streaming={session.streaming}
                              />
                            </>
                          }
                          status={
                            <time dateTime={session.modified}>
                              {relativeTime(session.modified, language)}
                            </time>
                          }
                          actions={
                            <DropdownMenu>
                              <DropdownMenuTrigger asChild>
                                <Button
                                  type="button"
                                  variant="ghost"
                                  size="icon"
                                  className="size-8 shrink-0 text-content-muted max-[650px]:size-11 pointer-coarse:size-11"
                                  aria-label={t('chat:chatHistoryPage.moreActionsForChat', {
                                    name,
                                  })}
                                >
                                  <MoreHorizontal size={16} />
                                </Button>
                              </DropdownMenuTrigger>
                              <DropdownMenuContent align="end" className="min-w-48">
                                <DropdownMenuItem
                                  className="min-h-11"
                                  onSelect={() => void renameSession(session)}
                                >
                                  <Pencil />
                                  {t('chat:chatHistoryPage.renameChat')}
                                </DropdownMenuItem>
                                <DropdownMenuItem
                                  className="min-h-11"
                                  onSelect={() =>
                                    void organizeSession(
                                      session,
                                      { pinned: !session.pinned },
                                      session.pinned
                                        ? t('chat:chatHistoryPage.unpinned')
                                        : t('chat:chatHistoryPage.pinned'),
                                    )
                                  }
                                >
                                  {session.pinned ? <PinOff /> : <Pin />}
                                  {session.pinned
                                    ? t('chat:chatHistoryPage.unpin')
                                    : t('chat:chatHistoryPage.pin')}
                                </DropdownMenuItem>
                                <DropdownMenuItem
                                  className="min-h-11"
                                  onSelect={() =>
                                    void organizeSession(
                                      session,
                                      { archived: !session.archived },
                                      session.archived
                                        ? t('chat:chatHistoryPage.restored')
                                        : t('chat:chatHistoryPage.movedToArchive'),
                                    )
                                  }
                                >
                                  {session.archived ? <ArchiveRestore /> : <Archive />}
                                  {session.archived
                                    ? t('chat:chatHistoryPage.restore')
                                    : t('chat:chatHistoryPage.archive')}
                                </DropdownMenuItem>
                                {session.unread && (
                                  <DropdownMenuItem
                                    className="min-h-11"
                                    onSelect={() =>
                                      void organizeSession(
                                        session,
                                        { read: true },
                                        t('chat:chatHistoryPage.markedRead'),
                                      )
                                    }
                                  >
                                    {t('chat:chatHistoryPage.markRead')}
                                  </DropdownMenuItem>
                                )}
                                <DropdownMenuSeparator />
                                <DropdownMenuItem
                                  variant="destructive"
                                  className="min-h-11"
                                  onSelect={() => void deleteSession(session)}
                                >
                                  <Trash2 />
                                  {t('chat:chatHistoryPage.deleteChat')}
                                </DropdownMenuItem>
                              </DropdownMenuContent>
                            </DropdownMenu>
                          }
                        />
                      )
                    })}
                  </FeatureList>
                </FeatureSection>
              ))}
              {visible.total > HISTORY_BATCH_SIZE && (
                <div className="flex justify-center pt-4">
                  {visible.total > visible.items.length ? (
                    <Button
                      type="button"
                      variant="secondary"
                      className="tabular-nums max-[650px]:min-h-11 pointer-coarse:min-h-11"
                      onClick={() => {
                        const nextCount = visibleCount + HISTORY_BATCH_SIZE
                        completedBatchFocusRef.current = nextCount >= visible.total
                        setPageLimit({ filterKey, count: nextCount })
                      }}
                    >
                      {t('chat:chatHistoryPage.loadMoreChats', {
                        shown: visible.items.length,
                        total: visible.total,
                      })}
                    </Button>
                  ) : (
                    <p
                      ref={completedBatchStatusRef}
                      tabIndex={-1}
                      role="status"
                      className="py-2 text-xs text-content-muted tabular-nums"
                    >
                      {t('chat:chatHistoryPage.allChatsShown', { count: visible.total })}
                    </p>
                  )}
                </div>
              )}
            </div>
          ) : (
            <PageState kind="empty" title={emptyTitle} description={emptyDescription} />
          )}
        </>
      )}
    </FeaturePage>
  )
}
