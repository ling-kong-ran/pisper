import { useCallback, useEffect, useState } from 'react'
import type { IDockviewPanelProps } from 'dockview-react'
import { ArrowLeft, ArrowRight, ExternalLink, Globe2, LoaderCircle, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Threads } from '@/components/react-bits'
import {
  WebPreview,
  WebPreviewBody,
  WebPreviewNavigation,
  WebPreviewNavigationButton,
  WebPreviewUrl,
} from '@/components/ai-elements/web-preview'
import { normalizeWebPreviewInput } from '@/lib/web-preview'
import { webPreviewPanelTitle, type WebPreviewPanelParams } from './web-preview-panel'

type PreviewHistory = {
  entries: string[]
  index: number
}

function historyForUrl(url: string): PreviewHistory {
  return url ? { entries: [url], index: 0 } : { entries: [], index: -1 }
}

export function WebPreviewDockPanel({ params, api }: IDockviewPanelProps<WebPreviewPanelParams>) {
  const { t } = useI18n()
  const incomingUrl = normalizeWebPreviewInput(params?.url || '', window.location.href) || ''
  const [visible, setVisible] = useState(() => api.isVisible)
  const [history, setHistory] = useState<PreviewHistory>(() => historyForUrl(incomingUrl))
  const [reloadKey, setReloadKey] = useState(0)
  const [loading, setLoading] = useState(Boolean(incomingUrl))
  const currentUrl = history.index >= 0 ? history.entries[history.index] || '' : ''

  useEffect(() => {
    setVisible(api.isVisible)
    const disposable = api.onDidVisibilityChange(({ isVisible }) => setVisible(isVisible))
    return () => disposable.dispose()
  }, [api])

  useEffect(() => {
    if (!incomingUrl || incomingUrl === currentUrl) return
    setHistory(historyForUrl(incomingUrl))
    setReloadKey(0)
    setLoading(true)
  }, [currentUrl, incomingUrl])

  useEffect(() => {
    api.setTitle(webPreviewPanelTitle(currentUrl, t('common:webPreview.title')))
  }, [api, currentUrl, t])

  const navigateTo = useCallback(
    (value: string) => {
      const normalized = normalizeWebPreviewInput(value, currentUrl || window.location.href)
      if (!normalized || normalized === currentUrl) return
      setHistory((current) => ({
        entries: [...current.entries.slice(0, current.index + 1), normalized],
        index: current.index + 1,
      }))
      api.updateParameters({ url: normalized })
      setReloadKey(0)
      setLoading(true)
    },
    [api, currentUrl],
  )

  const navigateHistory = useCallback(
    (offset: number) => {
      const index = history.index + offset
      if (index < 0 || index >= history.entries.length) return
      const url = history.entries[index]
      setHistory((current) => ({ ...current, index }))
      api.updateParameters({ url })
      setLoading(true)
    },
    [api, history],
  )

  const reload = useCallback(() => {
    setReloadKey((current) => current + 1)
    setLoading(true)
  }, [])

  const openExternal = useCallback(() => {
    if (currentUrl) window.open(currentUrl, '_blank', 'noopener,noreferrer')
  }, [currentUrl])

  if (!visible) return null

  if (!currentUrl) {
    return (
      <div className="web-preview-panel web-preview-empty" onFocusCapture={() => api.setActive()}>
        <Globe2 size={24} />
        <strong>{t('common:webPreview.title')}</strong>
      </div>
    )
  }

  return (
    <div className="web-preview-panel" onFocusCapture={() => api.setActive()}>
      <WebPreview
        className="rounded-none border-0"
        defaultUrl={currentUrl}
        key={`${history.index}:${currentUrl}`}
        onUrlChange={navigateTo}
      >
        <WebPreviewNavigation>
          <WebPreviewNavigationButton
            aria-label={t('common:webPreview.back')}
            disabled={history.index <= 0}
            onClick={() => navigateHistory(-1)}
            tooltip={t('common:webPreview.back')}
          >
            <ArrowLeft />
          </WebPreviewNavigationButton>
          <WebPreviewNavigationButton
            aria-label={t('common:webPreview.forward')}
            disabled={history.index >= history.entries.length - 1}
            onClick={() => navigateHistory(1)}
            tooltip={t('common:webPreview.forward')}
          >
            <ArrowRight />
          </WebPreviewNavigationButton>
          <WebPreviewNavigationButton
            aria-label={t('common:webPreview.reload')}
            onClick={reload}
            tooltip={t('common:webPreview.reload')}
          >
            <RefreshCw />
          </WebPreviewNavigationButton>
          <div className="web-preview-address">
            <Globe2 aria-hidden="true" />
            <WebPreviewUrl
              aria-label={t('common:webPreview.url')}
              placeholder={t('common:webPreview.urlPlaceholder')}
            />
          </div>
          <WebPreviewNavigationButton
            aria-label={t('common:webPreview.openExternal')}
            onClick={openExternal}
            tooltip={t('common:webPreview.openExternal')}
          >
            <ExternalLink />
          </WebPreviewNavigationButton>
        </WebPreviewNavigation>
        <WebPreviewBody
          key={`${currentUrl}:${reloadKey}`}
          onLoad={() => setLoading(false)}
          src={currentUrl}
          title={t('common:webPreview.title')}
          loading={
            loading ? (
              <div className="web-preview-loading">
                <Threads />
                <span className="web-preview-loading-copy">
                  <LoaderCircle className="spin" size={18} />
                  <span>{t('common:webPreview.loading')}</span>
                </span>
              </div>
            ) : null
          }
        />
        <div className="web-preview-notice">{t('common:webPreview.embedNotice')}</div>
      </WebPreview>
    </div>
  )
}
