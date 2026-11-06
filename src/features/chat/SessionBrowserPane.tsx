// 会话内的网页预览页：地址栏 + 网页 + 外部打开。旧上下文面板与会话页检查器的“预览”标签共用。
// 桌面端用原生子 WebView（useNativeBrowser），Web、手机和不支持的平台用 iframe。
import { useState } from 'react'
import { ArrowLeft, ArrowRight, ExternalLink, Globe2, LoaderCircle, RotateCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import { WebPreview, WebPreviewBody } from '@/components/ai-elements/web-preview'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { HTML_PREVIEW_SANDBOX, isHtmlPreviewUrl, normalizeWebPreviewInput } from '@/lib/web-preview'
import type { ContextPage } from './session-context-pages'
import {
  openBrowserPopupInSessionContext,
  openWebPreviewInSessionContext,
} from './session-web-preview'
import { useNativeBrowser } from './useNativeBrowser'

export function BrowserPane({
  sessionId,
  page,
  onChange,
  onClosed,
}: {
  sessionId: string
  page: ContextPage
  onChange: (patch: { draft?: string; url?: string; title?: string }) => void
  // 网页调用了 window.close()。
  onClosed: () => void
}) {
  const { t } = useI18n()
  const { draft, url } = page
  const [error, setError] = useState(false)
  const native = useNativeBrowser({
    enabled: Boolean(window.pisperDesktop?.browser),
    url,
    popupId: page.resourceId,
    // 用户正在输入时不改地址栏；只有地址栏显示的还是旧地址才跟着变。
    onNavigated: (next) => {
      if (next !== url) onChange(draft === url ? { url: next, draft: next } : { url: next })
    },
    onTitle: (title) => onChange({ title }),
    onNewWindow: (next, popupId) => {
      if (popupId && openBrowserPopupInSessionContext(sessionId, popupId, next)) return
      // 页签开满了：弹窗显示不出来，关掉它，地址在当前页签打开。
      if (popupId) void window.pisperDesktop?.browser?.close(popupId).catch(() => {})
      if (next) openWebPreviewInSessionContext(sessionId, next)
    },
    onClosed,
  })

  return (
    <section
      className="flex min-h-0 flex-1 flex-col gap-2 p-3"
      aria-label={t('common:webPreview.title')}
    >
      <form
        className="flex gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          const normalized = normalizeWebPreviewInput(draft, window.location.href)
          setError(!normalized)
          if (normalized) onChange({ url: normalized })
        }}
      >
        {native.active && (
          <div className="flex shrink-0 items-center">
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={t('common:webPreview.back')}
              title={t('common:webPreview.back')}
              onClick={() => native.act('back')}
            >
              <ArrowLeft size={14} />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={t('common:webPreview.forward')}
              title={t('common:webPreview.forward')}
              onClick={() => native.act('forward')}
            >
              <ArrowRight size={14} />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={t('common:webPreview.reload')}
              title={t('common:webPreview.reload')}
              aria-busy={native.loading}
              onClick={() => native.act('reload')}
            >
              {native.loading ? (
                <LoaderCircle size={14} className="animate-spin" />
              ) : (
                <RotateCw size={14} />
              )}
            </Button>
          </div>
        )}
        <Input
          aria-label={t('common:webPreview.url')}
          className="flex-1 font-mono"
          value={draft}
          onChange={(event) => onChange({ draft: event.target.value })}
          placeholder={t('common:webPreview.urlPlaceholder')}
          maxLength={2048}
        />
        <Button type="submit" variant="secondary">
          {t('chat:sessionContext.openUrl')}
        </Button>
      </form>
      {error && (
        <p role="alert" className="text-xs text-danger">
          {t('chat:sessionContext.invalidUrl')}
        </p>
      )}
      {native.active ? (
        <div
          ref={native.slotRef}
          role="group"
          aria-label={t('common:webPreview.title')}
          aria-busy={native.loading}
          data-native-browser-slot
          className="min-h-0 flex-1 overflow-hidden rounded-md border border-stroke bg-background"
        />
      ) : url ? (
        <WebPreview
          className="min-h-0 flex-1 overflow-hidden rounded-md border-stroke"
          defaultUrl={url}
          key={url}
        >
          <WebPreviewBody
            src={url}
            title={t('common:webPreview.title')}
            {...(isHtmlPreviewUrl(url) ? { sandbox: HTML_PREVIEW_SANDBOX } : {})}
          />
        </WebPreview>
      ) : (
        <InlineEmptyState
          icon={Globe2}
          className="min-h-0 flex-1"
          title={t('chat:sessionContext.browserEmpty')}
        />
      )}
      {url && (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="self-start"
          onClick={() => window.open(url, '_blank', 'noopener,noreferrer')}
        >
          <ExternalLink size={14} />
          {t('common:webPreview.openExternal')}
        </Button>
      )}
      {!native.active && (
        <p className="text-xs text-content-muted">{t('common:webPreview.embedNotice')}</p>
      )}
    </section>
  )
}
