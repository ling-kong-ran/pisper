// Web 预览 Dock 面板：内嵌浏览器（地址栏 + iframe），
// 支持前进/后退/刷新/新开，URL 状态跨会话持久。
import { lazy, Suspense, useCallback, useEffect, useState } from 'react'
import type { IDockviewPanelProps } from 'dockview-react'
import { ArrowLeft, ArrowRight, ExternalLink, Globe2, LoaderCircle, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  WebPreview,
  WebPreviewBody,
  WebPreviewNavigation,
  WebPreviewNavigationButton,
  WebPreviewUrl,
} from '@/components/ai-elements/web-preview'
import { normalizeWebPreviewInput } from '@/lib/web-preview'
import { webPreviewPanelTitle, type WebPreviewPanelParams } from './web-preview-panel'

const Threads = lazy(() =>
  import('@/components/react-bits/Threads').then((module) => ({ default: module.Threads })),
)

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
      <div
        className="web-preview-panel [&_>_div]:rounded-[0] w-full h-full min-w-0 min-h-0 overflow-hidden bg-[var(--panel)] web-preview-empty [&_strong]:text-[var(--text)] [&_strong]:text-[13px] flex flex-col items-center justify-center gap-[8px] text-[var(--text-muted)]"
        onFocusCapture={() => api.setActive()}
      >
        <Globe2 size={24} />
        <strong>{t('common:webPreview.title')}</strong>
      </div>
    )
  }

  return (
    <div
      className="web-preview-panel [&_>_div]:rounded-[0] w-full h-full min-w-0 min-h-0 overflow-hidden bg-[var(--panel)]"
      onFocusCapture={() => api.setActive()}
    >
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
          <div className="web-preview-address [&_>_svg]:w-[14px] [&_>_svg]:h-[14px] [&_>_svg]:flex-none [&_>_svg]:text-[var(--text-muted)] flex min-w-0 flex-1 items-center gap-[7px] [margin:0_3px]">
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
              <div className="absolute inset-0 grid place-items-center overflow-hidden bg-[color-mix(in_srgb,var(--panel)_86%,transparent)] text-[var(--text-muted)] text-[12px] [backdrop-filter:blur(3px)]">
                <Suspense fallback={null}>
                  <Threads />
                </Suspense>
                <span className="relative z-[1] flex items-center gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-pill)] bg-[color-mix(in_srgb,var(--panel)_82%,transparent)] [padding:7px_11px] shadow-[var(--sh-1)]">
                  <LoaderCircle className="animate-spin" size={18} />
                  <span>{t('common:webPreview.loading')}</span>
                </span>
              </div>
            ) : null
          }
        />
        <div className="flex-none [border-top:1px_solid_var(--stroke-soft)] bg-[var(--surface-subtle)] [padding:5px_10px] text-[var(--text-muted)] text-[10px] leading-[1.4]">
          {t('common:webPreview.embedNotice')}
        </div>
      </WebPreview>
    </div>
  )
}
