// 历史会话页：搜索/分页浏览历史会话列表，支持翻页与恢复。
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ChevronRight,
  History,
  MessageSquare,
  PanelBottom,
  PanelLeft,
  PanelRight,
  PanelTop,
  Pencil,
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
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { apiJson } from '@/lib/api'
import { relativeTime, workspaceName } from '@/lib/format'
import type { SessionSummary } from '@/types/chat'
import {
  ACTIVE_SESSION_CHANGED_EVENT,
  SESSIONS_UPDATED_EVENT,
  announceActiveSession,
  announceSessionsUpdated,
  requestSessionSelection,
} from './events'
import type { SessionOpenDisposition } from './dock-layout'

import { Button } from '@/components/ui/button'

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
  const [activeId, setActiveId] = useState(
    () => localStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    setLoading(true)
    setError('')
    try {
      const data = await apiJson<{ sessions?: SessionSummary[] }>('/api/sessions')
      setSessions(
        [...(data.sessions || [])].sort(
          (a, b) => Date.parse(b.modified || '') - Date.parse(a.modified || ''),
        ),
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    document.title = `${t('chat:chatHistoryPage.chatHistory')} · ${APP_NAME}`
    return () => {
      document.title = APP_NAME
    }
  }, [t])

  useEffect(() => {
    load()
    const refresh = () => load()
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

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (!needle) return sessions
    return sessions.filter((session) =>
      `${session.name || ''} ${session.firstMessage || ''} ${session.cwd || ''} ${session.model || ''}`
        .toLowerCase()
        .includes(needle),
    )
  }, [query, sessions])

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
      announceSessionsUpdated()
      notify(t('chat:chatHistoryPage.chatTitleUpdated'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
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
      announceSessionsUpdated()
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
                ? t('chat:chatHistoryPage.countCurrentlyFiltered', { count: visible.length })
                : t('chat:chatHistoryPage.sortedByMostRecentlyUpdated')}
            </small>
          </span>
        </div>
        <Button
          variant="outline"
          size="lg"
          className="bg-surface-subtle"
          onClick={load}
          disabled={loading}
        >
          <RefreshCw className={loading ? 'animate-spin' : ''} size={14} />
          {t('chat:chatHistoryPage.refresh')}
        </Button>
      </div>
      {error && <AppError>{error}</AppError>}
      {loading && !sessions.length ? (
        <AppEmptyState>
          <RefreshCw className="animate-spin" size={22} />
          <h2>{t('chat:chatHistoryPage.loadingChatHistory')}</h2>
        </AppEmptyState>
      ) : visible.length ? (
        <Panel className="chat-history-list overflow-hidden !p-[5px]">
          {visible.map((session) => {
            return (
              <SpotlightCard
                className={`chat-history-row [.chat-history-row_+_&]:[border-top:1px_solid_var(--stroke-soft)] hover:bg-[var(--surface-subtle)] [&.active]:bg-[var(--surface-subtle)] [&.active]:shadow-[inset_3px_0_var(--brand-blue)] grid grid-cols-[minmax(0,1fr)_auto] items-center rounded-[var(--r-sm)] ${session.id === activeId ? 'active' : ''}`}
                key={session.id}
              >
                <button
                  className="chat-history-open [&_>_svg]:text-[var(--text-muted)] max-[650px]:grid-cols-[32px_minmax(0,1fr)_auto] grid w-full min-w-0 min-h-[72px] grid-cols-[34px_minmax(0,1fr)_auto_auto] items-center gap-[10px] border-0 bg-transparent [padding:9px_10px] text-left"
                  onClick={() => openSession(session.id)}
                >
                  <span className="grid w-[32px] h-[32px] place-items-center rounded-[var(--r-sm)] bg-[var(--star-soft)] text-[var(--star-strong)]">
                    <MessageSquare size={15} />
                  </span>
                  <span className="chat-history-copy [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_>_span]:overflow-hidden [&_>_span]:text-ellipsis [&_>_span]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[var(--text)] [&_strong]:text-[13px] [&_>_span]:text-[var(--text-soft)] [&_>_span]:text-[12px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] max-[650px]:[&_>_span]:max-w-[54vw] flex min-w-0 flex-col gap-[3px]">
                    <strong title={session.name || t('chat:chatHistoryPage.untitledChat')}>
                      {session.name || t('chat:chatHistoryPage.untitledChat')}
                    </strong>
                    <span>
                      {session.firstMessage || t('chat:chatHistoryPage.noMessageSummary')}
                    </span>
                    <small>
                      {workspaceName(session.cwd, language)}
                      {session.model && !/(^|\/)unknown$/i.test(String(session.model))
                        ? ` · ${String(session.model).split('/').at(-1)}`
                        : ''}
                      {session.streaming ? ` · ${t('chat:chatHistoryPage.agentRunning')}` : ''}
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
                <div className="chat-history-actions [&_button]:grid [&_button]:w-[30px] [&_button]:h-[30px] [&_button]:min-h-[30px] [&_button]:place-items-center [&_button]:border-0 [&_button]:rounded-[var(--r-xs)] [&_button]:bg-transparent [&_button]:text-[var(--text-muted)] [&_button:hover]:bg-[var(--solid)] [&_button:hover]:text-[var(--text)] [&_button.active]:bg-[var(--star-soft)] [&_button.active]:text-[var(--star-strong)] [&_button.danger:hover]:bg-[var(--danger-soft)] [&_button.danger:hover]:text-[var(--danger)] max-[650px]:pr-[4px] flex items-center gap-[2px] [padding-right:8px]">
                  <button
                    title={t('chat:chatHistoryPage.splitToLeft')}
                    aria-label={t('chat:chatHistoryPage.splitNameToTheLeft', {
                      name: session.name,
                    })}
                    onClick={() => openSession(session.id, 'left')}
                  >
                    <PanelLeft size={14} />
                  </button>
                  <button
                    title={t('chat:chatHistoryPage.splitToRight')}
                    aria-label={t('chat:chatHistoryPage.splitNameToTheRight', {
                      name: session.name,
                    })}
                    onClick={() => openSession(session.id, 'right')}
                  >
                    <PanelRight size={14} />
                  </button>
                  <button
                    title={t('chat:chatHistoryPage.splitToTop')}
                    aria-label={t('chat:chatHistoryPage.splitNameToTheTop', {
                      name: session.name,
                    })}
                    onClick={() => openSession(session.id, 'above')}
                  >
                    <PanelTop size={14} />
                  </button>
                  <button
                    title={t('chat:chatHistoryPage.splitToBottom')}
                    aria-label={t('chat:chatHistoryPage.splitNameToTheBottom', {
                      name: session.name,
                    })}
                    onClick={() => openSession(session.id, 'below')}
                  >
                    <PanelBottom size={14} />
                  </button>
                  <button
                    title={t('chat:chatHistoryPage.renameChat')}
                    aria-label={t('chat:chatHistoryPage.renameChat')}
                    onClick={() => renameSession(session)}
                  >
                    <Pencil size={14} />
                  </button>
                  <button
                    className="danger"
                    title={t('chat:chatHistoryPage.deleteChat')}
                    aria-label={t('chat:chatHistoryPage.deleteChat')}
                    onClick={() => deleteSession(session)}
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              </SpotlightCard>
            )
          })}
        </Panel>
      ) : (
        <AppEmptyState>
          <StarOrbit size={48} />
          <h2>
            {query
              ? t('chat:chatHistoryPage.noMatchingConversations')
              : t('chat:chatHistoryPage.noConversationsHaveLeftAnEchoYet')}
          </h2>
          <p>
            {query
              ? t('chat:chatHistoryPage.tryADifferentSearch')
              : t('chat:chatHistoryPage.onceAConversationBeginsItWillBeQuietlyGatheredHere')}
          </p>
        </AppEmptyState>
      )}
    </div>
  )
}
