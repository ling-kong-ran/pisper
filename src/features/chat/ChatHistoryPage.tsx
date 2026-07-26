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
import { SpotlightCard } from '@/components/react-bits'
import { StarOrbit } from '@/components/StarOrbit'
import { Panel } from '@/components/ui'
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
    <div className="chat-history-page">
      <div className="chat-history-summary">
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
        <button className="button secondary" onClick={load} disabled={loading}>
          <RefreshCw className={loading ? 'spin' : ''} size={14} />
          {t('chat:chatHistoryPage.refresh')}
        </button>
      </div>
      {error && <div className="config-error">{error}</div>}
      {loading && !sessions.length ? (
        <Panel className="empty-state">
          <RefreshCw className="spin" size={22} />
          <h2>{t('chat:chatHistoryPage.loadingChatHistory')}</h2>
        </Panel>
      ) : visible.length ? (
        <Panel className="chat-history-list">
          {visible.map((session) => {
            return (
              <SpotlightCard
                className={`chat-history-row ${session.id === activeId ? 'active' : ''}`}
                key={session.id}
              >
                <button className="chat-history-open" onClick={() => openSession(session.id)}>
                  <span className="chat-history-icon">
                    <MessageSquare size={15} />
                  </span>
                  <span className="chat-history-copy">
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
                  <span className="chat-history-meta">
                    <strong>
                      {t('chat:chatHistoryPage.countMessages', {
                        count: session.messageCount || 0,
                      })}
                    </strong>
                    <small>{relativeTime(session.modified, language)}</small>
                  </span>
                  <ChevronRight size={15} />
                </button>
                <div className="chat-history-actions">
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
        <Panel className="empty-state">
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
        </Panel>
      )}
    </div>
  )
}
