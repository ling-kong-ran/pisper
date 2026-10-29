// 历史会话页：搜索完整会话目录并分批展示，支持继续打开和管理。
// 列表按置顶/今天/近 7 天/更早分组；桌面每组是一张细线卡片，手机为平铺列表与内缩分隔线。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Archive,
  ArchiveRestore,
  FolderClosed,
  MoreHorizontal,
  Pencil,
  Pin,
  PinOff,
  RefreshCw,
  Trash2,
} from 'lucide-react'
import { APP_NAME } from '@/app/brand'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { STORAGE_KEYS } from '@/app/storage'
import { pageStateStorage } from '@/lib/page-state-storage'
import { SpotlightCard } from '@/components/react-bits/SpotlightCard'
import { PageState } from '@/components/app/PageState'
import { AppError } from '@/components/ui/app-primitives'
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
import { fetchStartupQuery } from '@/lib/startup-queries'
import type { SessionSummary } from '@/types/chat'
import {
  ACTIVE_SESSION_CHANGED_EVENT,
  SESSIONS_UPDATED_EVENT,
  announceActiveSession,
  announceSessionsUpdated,
  requestSessionSelection,
  sessionDeletionUpdateFromEvent,
  sessionOrganizationUpdateFromEvent,
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

type ChatHistoryPageProps = {
  query: string
  navigate: (page: string) => void
  notify: Notify
  requestConfirm: (options: ConfirmDialogOptions) => Promise<boolean>
  requestText: (options: PromptDialogOptions) => Promise<string | null>
}

export function ChatHistoryPage({
  query,
  navigate,
  notify,
  requestConfirm,
  requestText,
}: ChatHistoryPageProps) {
  const { t, language } = useI18n()
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [historyView, setHistoryView] = useState<HistoryView>('active')
  const filterKey = `${historyView}\0${query}`
  const [pageLimit, setPageLimit] = useState({ filterKey, count: HISTORY_BATCH_SIZE })
  const loadGenerationRef = useRef(0)
  const completedBatchFocusRef = useRef(false)
  const completedBatchStatusRef = useRef<HTMLParagraphElement>(null)
  const [activeId, setActiveId] = useState(
    () => pageStateStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  useEffect(() => {
    setPageLimit((current) =>
      current.filterKey === filterKey && current.count === HISTORY_BATCH_SIZE
        ? current
        : { filterKey, count: HISTORY_BATCH_SIZE },
    )
  }, [filterKey])

  const load = useCallback(async (silent = false) => {
    const generation = ++loadGenerationRef.current
    if (!silent) {
      setLoading(true)
      setError('')
    }
    try {
      const data = await fetchStartupQuery<{ sessions?: SessionSummary[] }>('sessions', true)
      if (generation !== loadGenerationRef.current) return
      setSessions(orderVisibleSessions(data.sessions || []))
    } catch (caught) {
      if (generation === loadGenerationRef.current)
        setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      if (generation === loadGenerationRef.current) setLoading(false)
    }
  }, [])

  useEffect(() => {
    document.title = `${t('chat:chatHistoryPage.chatHistory')} · ${APP_NAME}`
    return () => {
      document.title = APP_NAME
    }
  }, [t])

  useEffect(() => {
    void load()
    const refresh = (event: Event) => {
      const deletion = sessionDeletionUpdateFromEvent(event)
      if (deletion) {
        const deleted = new Set(deletion.deletedIds)
        setSessions((current) => current.filter((session) => !deleted.has(session.id)))
      }
      const organization = sessionOrganizationUpdateFromEvent(event)
      if (organization)
        setSessions((current) => applySessionOrganizationUpdate(current, organization))
      void load(true)
    }
    const syncActive = (event: Event) =>
      setActiveId(
        (event as CustomEvent<{ id?: string }>).detail?.id ||
          pageStateStorage.getItem(STORAGE_KEYS.activeSession) ||
          '',
      )
    window.addEventListener(SESSIONS_UPDATED_EVENT, refresh)
    window.addEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
    return () => {
      window.removeEventListener(SESSIONS_UPDATED_EVENT, refresh)
      window.removeEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
    }
  }, [load])

  // 会话可在聊天页之外由后台任务结算；历史页停留期间定期读取真实状态。
  useEffect(() => {
    const refreshVisible = () => {
      if (document.visibilityState === 'visible') void load(true)
    }
    const timer = window.setInterval(refreshVisible, 20_000)
    document.addEventListener('visibilitychange', refreshVisible)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', refreshVisible)
    }
  }, [load])

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
      setSessions((current) =>
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
      setSessions((current) => applySessionOrganizationUpdate(current, updated))
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
      setSessions(remaining)
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

  // 状态徽标只反映目录里已有的运行、失败、待审批与需处理字段，不推测其它状态。
  const statusBadge = (session: SessionSummary) => {
    if (session.streaming)
      return (
        <Badge variant="run" className="gap-1.5">
          <OrbitSpinner />
          {t('chat:chatHistoryPage.statusRunning')}
        </Badge>
      )
    if (!session.needsAttention) return null
    if (session.attentionReason === 'failure')
      return <Badge variant="bad">{t('chat:chatHistoryPage.statusFailed')}</Badge>
    return (
      <Badge variant="warn">
        {session.attentionReason === 'approval'
          ? t('chat:chatHistoryPage.statusApproval')
          : t('chat:chatHistoryPage.needsAttention')}
      </Badge>
    )
  }

  return (
    <div className="mx-auto flex min-h-[100%] w-full max-w-[900px] flex-col gap-3 max-[650px]:gap-2">
      {/* 分段控件与统计同一行：选中段抬起为键帽，计数用等宽数字；手机上分段铺满整行。 */}
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
                // 手机与粗指针下每段本身就是 44px 触控目标，轨道随之撑高。
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
      {error && <AppError>{error}</AppError>}
      {loading && !sessions.length ? (
        <PageState kind="loading" title={t('chat:chatHistoryPage.loadingChatHistory')} />
      ) : visible.total ? (
        <div className="chat-history-list flex flex-col gap-1">
          {visible.groups.map((group) => (
            <section key={group.key} aria-labelledby={`chat-history-group-${group.key}`}>
              {/* 分组标题 12/500 弱化文字，右侧是该组在完整筛选结果中的会话数。 */}
              <h2
                id={`chat-history-group-${group.key}`}
                className="flex h-[34px] items-end justify-between px-1 pb-1.5 text-xs font-medium tracking-[.02em] text-content-muted"
              >
                <span>{groupLabels[group.key]}</span>
                <span className="font-normal tabular-nums">{group.count}</span>
              </h2>
              <ul className="divide-y divide-stroke-soft overflow-hidden rounded-md border border-stroke bg-panel max-[650px]:rounded-none max-[650px]:border-0 max-[650px]:bg-transparent">
                {group.items.map((session) => {
                  const name = session.name || t('chat:chatHistoryPage.untitledChat')
                  const model =
                    session.model && !/(^|\/)unknown$/i.test(String(session.model))
                      ? String(session.model).split('/').at(-1)
                      : ''
                  return (
                    <li key={session.id}>
                      {/* 聚光层的径向高光不属于本设计语言，这里只保留卡片容器并关闭它。 */}
                      <SpotlightCard
                        className={cn(
                          'chat-history-row grid grid-cols-[minmax(0,1fr)_auto] items-center transition-colors duration-(--d-fast) before:hidden! hover:bg-surface-subtle active:bg-surface-subtle',
                          session.id === activeId &&
                            'active bg-surface-subtle after:absolute after:top-3 after:bottom-3 after:left-0 after:w-[3px] after:rounded-r-[3px] after:bg-star',
                        )}
                      >
                        <button
                          className="chat-history-open flex min-w-0 flex-col py-2.5 pr-1 pl-3 text-left focus-visible:-outline-offset-2 max-[650px]:pl-1"
                          type="button"
                          aria-label={t('chat:chatHistoryPage.openChatNamed', { name })}
                          onClick={() => openSession(session.id)}
                        >
                          <span className="chat-history-copy flex min-w-0 flex-col gap-0.5">
                            <span className="flex min-w-0 items-center gap-2">
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
                                className="min-w-0 flex-1 truncate text-base leading-[1.45] font-medium text-content max-[650px]:text-[15px]"
                                title={name}
                              >
                                {name}
                              </strong>
                              <time
                                className="shrink-0 text-xs text-content-muted tabular-nums"
                                dateTime={session.modified}
                              >
                                {relativeTime(session.modified, language)}
                              </time>
                            </span>
                            <span
                              className={cn(
                                'truncate text-sm leading-5',
                                session.firstMessage
                                  ? 'text-content-secondary'
                                  : 'text-content-muted',
                              )}
                            >
                              {session.firstMessage || t('chat:chatHistoryPage.noMessageSummary')}
                            </span>
                            <span className="mt-0.5 flex min-w-0 items-center gap-2 text-xs leading-[1.5] text-content-muted">
                              {statusBadge(session)}
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
                            </span>
                          </span>
                        </button>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon"
                              className="mr-2 size-8 shrink-0 text-content-muted max-[650px]:size-11 pointer-coarse:size-11"
                              aria-label={t('chat:chatHistoryPage.moreActionsForChat', { name })}
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
                      </SpotlightCard>
                    </li>
                  )
                })}
              </ul>
            </section>
          ))}
          {visible.total > HISTORY_BATCH_SIZE && (
            <div className="flex justify-center pt-2">
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
        // 空状态沿用全页状态组件（h2 标题供辅助技术定位），外加 1px 虚线卡片承载星轨插画。
        <div className="mt-2 rounded-xl border border-dashed border-stroke-strong">
          <PageState
            kind="empty"
            title={
              query
                ? t('chat:chatHistoryPage.noMatchingConversations')
                : historyView === 'archived'
                  ? t('chat:chatHistoryPage.noArchivedChats')
                  : sessions.length
                    ? t('chat:chatHistoryPage.noActiveChats')
                    : t('chat:chatHistoryPage.noConversationsHaveLeftAnEchoYet')
            }
            description={
              query
                ? t('chat:chatHistoryPage.tryADifferentSearch')
                : historyView === 'archived'
                  ? t('chat:chatHistoryPage.archivedChatsAppearHere')
                  : sessions.length
                    ? t('chat:chatHistoryPage.restoreArchivedChats')
                    : t('chat:chatHistoryPage.onceAConversationBeginsItWillBeQuietlyGatheredHere')
            }
          />
        </div>
      )}
    </div>
  )
}
