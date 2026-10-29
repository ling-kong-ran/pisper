// Web 预览 Dock 面板：内嵌浏览器（地址栏 + iframe），
// 支持前进/后退/刷新/新开，URL 状态跨会话持久。
import { lazy, Suspense, useCallback, useEffect, useState } from 'react'
import type { IDockviewPanelProps } from 'dockview-react'
import { ArrowLeft, ArrowRight, ExternalLink, Globe2, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { OrbitSpinner } from '@/components/ui/spinner'
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
        className="web-preview-panel web-preview-empty flex h-full min-h-0 w-full min-w-0 flex-col items-center justify-center gap-2 overflow-hidden bg-panel text-content-muted [&_strong]:text-sm [&_strong]:font-medium [&_strong]:text-content"
        onFocusCapture={() => api.setActive()}
      >
        <span className="grid size-8 place-items-center rounded-md bg-key text-content-secondary shadow-key ring-1 ring-key-border">
          <Globe2 className="size-4" aria-hidden="true" />
        </span>
        <strong>{t('common:webPreview.title')}</strong>
      </div>
    )
  }

  return (
    <div
      className="web-preview-panel h-full min-h-0 w-full min-w-0 overflow-hidden bg-panel [&_>_div]:rounded-none"
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
          <div className="web-preview-address mx-0.5 flex min-w-0 flex-1 items-center gap-2 [&_>_svg]:size-3.5 [&_>_svg]:flex-none [&_>_svg]:text-content-muted">
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
              // 加载层不透明、不做背景模糊；状态胶囊用浮层投影与品牌轨道指示。
              <div className="absolute inset-0 grid place-items-center overflow-hidden bg-panel text-xs text-content-muted">
                <Suspense fallback={null}>
                  <Threads />
                </Suspense>
                <span className="relative z-[1] flex items-center gap-2 rounded-full bg-popover px-3 py-1.5 shadow-float">
                  <OrbitSpinner className="size-3.5" />
                  <span>{t('common:webPreview.loading')}</span>
                </span>
              </div>
            ) : null
          }
        />
        <div className="flex-none border-t border-stroke-soft bg-surface-subtle px-2.5 py-1 text-xs leading-snug text-content-muted">
          {t('common:webPreview.embedNotice')}
        </div>
      </WebPreview>
    </div>
  )
}
