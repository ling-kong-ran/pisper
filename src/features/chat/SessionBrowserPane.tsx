// 会话内的网页预览页：地址栏 + 内嵌预览 + 外部打开。旧上下文面板与会话页检查器的“预览”标签共用。
import { useState } from 'react'
import { ExternalLink, Globe2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import { WebPreview, WebPreviewBody } from '@/components/ai-elements/web-preview'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { normalizeWebPreviewInput } from '@/lib/web-preview'
import type { ContextPage } from './session-context-pages'

export function BrowserPane({
  page,
  onChange,
}: {
  page: ContextPage
  onChange: (patch: { draft?: string; url?: string }) => void
}) {
  const { t } = useI18n()
  const { draft, url } = page
  const [error, setError] = useState(false)

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
        <Input
          aria-label={t('common:webPreview.url')}
          className="flex-1 font-mono"
          value={draft}
          onChange={(event) => onChange({ draft: event.target.value })}
          placeholder={t('common:webPreview.urlPlaceholder')}
          maxLength={2048}
        />
        <Button type="submit" variant="secondary" className="h-8.5 pointer-coarse:h-11">
          {t('chat:sessionContext.openUrl')}
        </Button>
      </form>
      {error && (
        <p role="alert" className="text-xs text-danger">
          {t('chat:sessionContext.invalidUrl')}
        </p>
      )}
      {url ? (
        <WebPreview
          className="min-h-0 flex-1 overflow-hidden rounded-md border-stroke"
          defaultUrl={url}
          key={url}
        >
          <WebPreviewBody src={url} title={t('common:webPreview.title')} />
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
      <p className="text-xs text-content-muted">{t('common:webPreview.embedNotice')}</p>
    </section>
  )
}
