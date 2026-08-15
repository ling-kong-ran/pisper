import { useCallback, useEffect, useMemo, useState } from 'react'
import { RefreshCw, Tag, Trash2 } from 'lucide-react'
import { APP_NAME } from '@/app/brand'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { AppCard as Panel } from '@/components/ui/app-primitives'
import { SpotlightCard } from '@/components/react-bits/SpotlightCard'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { relativeTime } from '@/lib/format'
import { chatErrorMessage } from './chat-errors'
import { chatApi, type SessionTreeLabelMatch } from './chat-api'
import { requestSessionSelection } from './events'

type LabelsPageProps = {
  query: string
  navigate: (page: string) => void
  notify: Notify
  requestConfirm: (options: ConfirmDialogOptions) => Promise<boolean>
}

export function LabelsPage({ query, navigate, notify, requestConfirm }: LabelsPageProps) {
  const { t, language } = useI18n()
  const [labels, setLabels] = useState<SessionTreeLabelMatch[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [deletingId, setDeletingId] = useState('')

  const load = useCallback(async () => {
    setLoading(true)
    setError('')
    try {
      const data = await chatApi.listSessionTreeLabels(500)
      setLabels(data.labels || [])
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const visible = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase(language)
    if (!needle) return labels
    return labels.filter((label) =>
      `${label.label} ${label.sessionName}`.toLocaleLowerCase(language).includes(needle),
    )
  }, [labels, language, query])

  const openLabel = async (label: SessionTreeLabelMatch) => {
    try {
      if (!label.active) {
        await chatApi.navigateSessionTree(label.sessionId, label.entryId, false)
      }
      requestSessionSelection(label.sessionId, 'open', label.entryId)
      navigate('chat')
    } catch {
      notify(t('navigation:appOverlays.openLabeledTurnFailed'), 'error')
    }
  }

  const removeLabel = async (label: SessionTreeLabelMatch) => {
    if (deletingId) return
    const approved = await requestConfirm({
      title: t('chat:labelsPage.delete'),
      message: t('chat:labelsPage.deleteConfirm', { label: label.label }),
      confirmLabel: t('chat:labelsPage.delete'),
      tone: 'danger',
    })
    if (!approved) return
    setDeletingId(`${label.sessionId}:${label.entryId}`)
    setError('')
    try {
      await chatApi.setSessionTreeLabel(label.sessionId, label.entryId, '')
      setLabels((current) =>
        current.filter(
          (item) => !(item.sessionId === label.sessionId && item.entryId === label.entryId),
        ),
      )
      notify(t('chat:labelsPage.deleteSuccess'))
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setDeletingId('')
    }
  }

  const formatTime = (value: string) => {
    const timestamp = Date.parse(value)
    if (!Number.isFinite(timestamp)) return t('navigation:appOverlays.unknownTime')
    return new Intl.DateTimeFormat(language, {
      dateStyle: 'medium',
      timeStyle: 'short',
    }).format(timestamp)
  }

  return (
    <div className="chat-history-page labels-page">
      <div className="chat-history-summary">
        <div>
          <Tag size={18} />
          <span>
            <strong>{t('chat:labelsPage.countItems', { count: labels.length })}</strong>
            <small>
              {query
                ? t('chat:chatHistoryPage.countCurrentlyFiltered', { count: visible.length })
                : t('chat:labelsPage.description')}
            </small>
          </span>
        </div>
        <button className="button secondary" onClick={() => void load()} disabled={loading}>
          <RefreshCw className={loading ? 'spin' : ''} size={14} />
          {t('chat:chatHistoryPage.refresh')}
        </button>
      </div>
      {error && <div className="config-error">{error}</div>}
      {loading && !labels.length ? (
        <Panel className="empty-state">
          <RefreshCw className="spin" size={22} />
          <h2>{t('chat:labelsPage.loading')}</h2>
        </Panel>
      ) : visible.length ? (
        <Panel className="chat-history-list">
          {visible.map((label) => {
            const key = `${label.sessionId}:${label.entryId}`
            const deleting = deletingId === key
            return (
              <SpotlightCard
                className={`chat-history-row${label.active ? ' active' : ''}`}
                key={key}
              >
                <button
                  type="button"
                  className="chat-history-open"
                  onClick={() => void openLabel(label)}
                >
                  <span className="chat-history-icon">
                    <Tag size={15} />
                  </span>
                  <span className="chat-history-copy">
                    <strong title={label.label}>{label.label}</strong>
                    <span>{label.sessionName || t('navigation:appOverlays.untitledChat')}</span>
                    <small>
                      {t('chat:labelsPage.labelTimes', {
                        sessionTime: formatTime(label.sessionModified),
                        nodeTime: formatTime(label.nodeTimestamp),
                      })}
                    </small>
                  </span>
                  <span className="chat-history-meta">
                    <strong>{label.active ? t('chat:labelsPage.active') : ''}</strong>
                    <small>{relativeTime(label.sessionModified, language)}</small>
                  </span>
                </button>
                <div className="chat-history-actions">
                  <button
                    type="button"
                    className="icon-button"
                    disabled={deleting}
                    aria-label={t('chat:labelsPage.delete')}
                    title={t('chat:labelsPage.delete')}
                    onClick={() => void removeLabel(label)}
                  >
                    {deleting ? <RefreshCw className="spin" size={14} /> : <Trash2 size={14} />}
                  </button>
                </div>
              </SpotlightCard>
            )
          })}
        </Panel>
      ) : (
        <Panel className="empty-state">
          <Tag size={22} />
          <h2>{APP_NAME}</h2>
          <p>{t('chat:labelsPage.empty')}</p>
        </Panel>
      )}
    </div>
  )
}
