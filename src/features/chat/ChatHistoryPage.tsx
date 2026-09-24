// 历史会话页：搜索完整会话目录并分批展示，支持继续打开和管理。
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import {
  Archive,
  ArchiveRestore,
  ChevronRight,
  CircleAlert,
  History,
  MessageSquare,
  MoreHorizontal,
  PanelBottom,
  PanelLeft,
  PanelRight,
  PanelTop,
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
import { SpotlightCard } from '@/components/react-bits/SpotlightCard'
import { StarOrbit } from '@/components/StarOrbit'
import { AppCard as Panel, AppError, AppEmptyState } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
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
import { useIsMobileApp } from '@/stores/client-store'
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
import type { SessionOpenDisposition } from './dock-layout'
import {
  canSplitHistorySessions,
  HISTORY_BATCH_SIZE,
  selectHistorySessions,
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

const compactDockQuery = '(max-width: 900px)'

function subscribeCompactDock(onChange: () => void) {
  const media = window.matchMedia(compactDockQuery)
  media.addEventListener('change', onChange)
  return () => media.removeEventListener('change', onChange)
}

function compactDockSnapshot() {
  return window.matchMedia(compactDockQuery).matches
}

export function ChatHistoryPage({
  query,
  navigate,
  notify,
  requestConfirm,
  requestText,
}: ChatHistoryPageProps) {
  const { t, language } = useI18n()
  const mobileApp = useIsMobileApp()
  const compactDock = useSyncExternalStore(subscribeCompactDock, compactDockSnapshot, () => true)
  const canSplit = canSplitHistorySessions(compactDock, mobileApp)
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [historyView, setHistoryView] = useState<HistoryView>('active')
  const filterKey = `${historyView}\0${query}`
  const [pageLimit, setPageLimit] = useState({ filterKey, count: HISTORY_BATCH_SIZE })
  const loadGenerationRef = useRef(0)
  const completedBatchFocusRef = useRef(false)
  const completedBatchStatusRef = useRef<HTMLParagraphElement>(null)
  const [activeId, setActiveId] = useState(
    () => localStorage.getItem(STORAGE_KEYS.activeSession) || '',
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
          localStorage.getItem(STORAGE_KEYS.activeSession) ||
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

  useEffect(() => {
    if (!completedBatchFocusRef.current) return
    completedBatchFocusRef.current = false
    completedBatchStatusRef.current?.focus()
  }, [visible.items.length, visible.total])

  const openSession = (id: string, disposition: SessionOpenDisposition = 'open') => {
    requestSessionSelection(id, disposition)
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
        if (nextId) localStorage.setItem(STORAGE_KEYS.activeSession, nextId)
        else localStorage.removeItem(STORAGE_KEYS.activeSession)
        announceActiveSession(nextId)
      }
      announceSessionsUpdated({ deletedIds: [session.id] })
      notify(t('chat:chatHistoryPage.chatDeleted'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  return (
    <div className="flex min-h-[100%] flex-col gap-[12px]">
      <div className="chat-history-summary [&_>_div]:flex [&_>_div]:min-w-0 [&_>_div]:items-center [&_>_div]:gap-[10px] [&_>_div]:text-[var(--star-strong)] [&_>_div_>_span]:flex [&_>_div_>_span]:min-w-0 [&_>_div_>_span]:flex-col [&_>_div_>_span]:gap-[2px] [&_strong]:text-[var(--text)] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] flex min-h-[52px] items-center justify-between gap-[12px] [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--panel)] [padding:8px_10px_8px_14px] shadow-[var(--sh-1)]">
        <div>
          <History size={18} />
          <span>
            <strong>
              {t('chat:chatHistoryPage.countChatHistoryItems', { count: sessions.length })}
            </strong>
            <small>
              {query
                ? t('chat:chatHistoryPage.countCurrentlyFiltered', { count: visible.total })
                : t('chat:chatHistoryPage.sortedByMostRecentlyUpdated')}
            </small>
          </span>
        </div>
        <Button
          variant="outline"
          size="lg"
          className="bg-surface-subtle"
          onClick={() => void load()}
          disabled={loading}
        >
          <RefreshCw className={loading ? 'animate-spin' : ''} size={14} />
          {t('chat:chatHistoryPage.refresh')}
        </Button>
      </div>
      <div
        role="group"
        aria-label={t('chat:chatHistoryPage.historySections')}
        className="flex gap-2"
      >
        {(['active', 'archived'] as const).map((view) => (
          <Button
            key={view}
            aria-pressed={historyView === view}
            variant={historyView === view ? 'secondary' : 'ghost'}
            className="min-h-11"
            onClick={() => setHistoryView(view)}
          >
            {view === 'active'
              ? t('chat:chatHistoryPage.activeChats')
              : t('chat:chatHistoryPage.archivedChats', { count: archivedCount })}
          </Button>
        ))}
      </div>
      {error && <AppError>{error}</AppError>}
      {loading && !sessions.length ? (
        <AppEmptyState>
          <RefreshCw className="animate-spin" size={22} />
          <h2>{t('chat:chatHistoryPage.loadingChatHistory')}</h2>
        </AppEmptyState>
      ) : visible.total ? (
        <Panel className="chat-history-list overflow-hidden !p-[5px]">
          {visible.items.map((session) => {
            return (
              <SpotlightCard
                className={`chat-history-row [.chat-history-row_+_&]:[border-top:1px_solid_var(--stroke-soft)] hover:bg-[var(--surface-subtle)] [&.active]:bg-[var(--surface-subtle)] [&.active]:shadow-[inset_3px_0_var(--brand-blue)] grid grid-cols-[minmax(0,1fr)_auto] items-center rounded-[var(--r-sm)] ${session.id === activeId ? 'active' : ''}`}
                key={session.id}
              >
                <button
                  className="chat-history-open [&_>_svg]:text-[var(--text-muted)] max-[650px]:grid-cols-[32px_minmax(0,1fr)_auto] grid w-full min-w-0 min-h-[72px] grid-cols-[34px_minmax(0,1fr)_auto_auto] items-center gap-[10px] border-0 bg-transparent [padding:9px_10px] text-left"
                  type="button"
                  aria-label={t('chat:chatHistoryPage.openChatNamed', {
                    name: session.name || t('chat:chatHistoryPage.untitledChat'),
                  })}
                  onClick={() => openSession(session.id)}
                >
                  <span className="grid w-[32px] h-[32px] place-items-center rounded-[var(--r-sm)] bg-[var(--star-soft)] text-[var(--star-strong)]">
                    <MessageSquare size={15} />
                  </span>
                  <span className="chat-history-copy [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_>_span]:overflow-hidden [&_>_span]:text-ellipsis [&_>_span]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[var(--text)] [&_strong]:text-[13px] [&_>_span]:text-[var(--text-soft)] [&_>_span]:text-[12px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] max-[650px]:[&_>_span]:max-w-[54vw] flex min-w-0 flex-col gap-[3px]">
                    <strong title={session.name || t('chat:chatHistoryPage.untitledChat')}>
                      {session.pinned && <Pin className="mr-1 inline size-3" aria-hidden="true" />}
                      {session.name || t('chat:chatHistoryPage.untitledChat')}
                    </strong>
                    <span>
                      {session.firstMessage || t('chat:chatHistoryPage.noMessageSummary')}
                    </span>
                    <small>
                      {session.needsAttention && (
                        <span className="mr-2 inline-flex items-center gap-1 text-[var(--text-secondary)]">
                          <CircleAlert className="size-3" aria-hidden="true" />
                          {t('chat:chatHistoryPage.needsAttention')}
                        </span>
                      )}
                      {session.unread && (
                        <span className="mr-2 font-semibold text-[var(--brand-blue)]">
                          {t('chat:chatHistoryPage.unread')}
                        </span>
                      )}
                      {session.archived && query.trim() && (
                        <span className="mr-2">{t('chat:chatHistoryPage.archived')}</span>
                      )}
                      {workspaceName(session.cwd, language)}
                      {session.model && !/(^|\/)unknown$/i.test(String(session.model))
                        ? ` · ${String(session.model).split('/').at(-1)}`
                        : ''}
                      {session.streaming ? ` · ${t('chat:chatHistoryPage.agentRunning')}` : ''}
                      <SessionChangeIndicator
                        sessionId={session.id}
                        lastCompletedAt={session.lastCompletedAt}
                        streaming={session.streaming}
                      />
                    </small>
                  </span>
                  <span className="chat-history-meta [&_strong]:text-[var(--text-soft)] [&_strong]:text-[11px] [&_small]:text-[var(--text-muted)] [&_small]:text-[10px] max-[650px]:hidden flex min-w-[92px] flex-col items-end gap-[3px]">
                    <strong>
                      {t('chat:chatHistoryPage.countMessages', {
                        count: session.messageCount || 0,
                      })}
                    </strong>
                    <small>{relativeTime(session.modified, language)}</small>
                  </span>
                  <ChevronRight size={15} />
                </button>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="mr-2 size-11 shrink-0"
                      aria-label={t('chat:chatHistoryPage.moreActionsForChat', {
                        name: session.name || t('chat:chatHistoryPage.untitledChat'),
                      })}
                    >
                      <MoreHorizontal size={18} />
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
                    {canSplit && (
                      <>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem
                          className="min-h-11"
                          onSelect={() => openSession(session.id, 'left')}
                        >
                          <PanelLeft />
                          {t('chat:chatHistoryPage.splitToLeft')}
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          className="min-h-11"
                          onSelect={() => openSession(session.id, 'right')}
                        >
                          <PanelRight />
                          {t('chat:chatHistoryPage.splitToRight')}
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          className="min-h-11"
                          onSelect={() => openSession(session.id, 'above')}
                        >
                          <PanelTop />
                          {t('chat:chatHistoryPage.splitToTop')}
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          className="min-h-11"
                          onSelect={() => openSession(session.id, 'below')}
                        >
                          <PanelBottom />
                          {t('chat:chatHistoryPage.splitToBottom')}
                        </DropdownMenuItem>
                      </>
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
            )
          })}
          {visible.total > HISTORY_BATCH_SIZE && (
            <div className="flex justify-center border-t border-border p-3">
              {visible.total > visible.items.length ? (
                <Button
                  type="button"
                  variant="outline"
                  className="min-h-11"
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
                  className="py-2 text-sm text-muted-foreground"
                >
                  {t('chat:chatHistoryPage.allChatsShown', { count: visible.total })}
                </p>
              )}
            </div>
          )}
        </Panel>
      ) : (
        <AppEmptyState>
          <StarOrbit size={48} />
          <h2>
            {query
              ? t('chat:chatHistoryPage.noMatchingConversations')
              : historyView === 'archived'
                ? t('chat:chatHistoryPage.noArchivedChats')
                : sessions.length
                  ? t('chat:chatHistoryPage.noActiveChats')
                  : t('chat:chatHistoryPage.noConversationsHaveLeftAnEchoYet')}
          </h2>
          <p>
            {query
              ? t('chat:chatHistoryPage.tryADifferentSearch')
              : historyView === 'archived'
                ? t('chat:chatHistoryPage.archivedChatsAppearHere')
                : sessions.length
                  ? t('chat:chatHistoryPage.restoreArchivedChats')
                  : t('chat:chatHistoryPage.onceAConversationBeginsItWillBeQuietlyGatheredHere')}
          </p>
        </AppEmptyState>
      )}
    </div>
  )
}
