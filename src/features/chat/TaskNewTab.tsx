import { useState } from 'react'
import { Input } from '@/components/ui/input'
import { normalizeWebPreviewInput } from '@/lib/web-preview'
import type { LucideIcon } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useI18n } from '@/app/use-i18n'
import type { SessionContextTab } from './session-context-pages'

export function TaskNewTab({
  tools,
  onSelect,
  onOpenUrl,
}: {
  tools: Array<{ id: SessionContextTab; label: string; icon: LucideIcon; disabled?: boolean }>
  onOpenUrl: (url: string) => void
  onSelect: (kind: SessionContextTab) => void
}) {
  const { t } = useI18n()
  const [url, setUrl] = useState('')
  const [invalid, setInvalid] = useState(false)
  return (
    <section className="@container/new-tab flex min-h-0 flex-1 flex-col overflow-y-auto bg-panel p-4">
      <h2 className="mb-3 text-sm font-medium text-content">{t('chat:workbench.newTab')}</h2>
      <form
        className="mb-4 flex gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          const normalized = normalizeWebPreviewInput(url, window.location.href)
          setInvalid(!normalized)
          if (normalized) onOpenUrl(normalized)
        }}
      >
        <Input
          className="font-mono"
          aria-label={t('common:webPreview.url')}
          placeholder={t('common:webPreview.urlPlaceholder')}
          value={url}
          maxLength={2048}
          onChange={(event) => setUrl(event.target.value)}
        />
        <Button type="submit" variant="secondary" className="h-8.5 pointer-coarse:h-11">
          {t('chat:sessionContext.openUrl')}
        </Button>
      </form>
      {invalid && (
        <p role="alert" className="mb-3 text-xs text-danger">
          {t('chat:sessionContext.invalidUrl')}
        </p>
      )}
      {/* 页面种类是平铺在细线框里的工具行：图标弱化、文字 13px，悬停只换底色，不抢强调色。 */}
      <div className="grid grid-cols-1 gap-1.5 @min-[420px]/new-tab:grid-cols-2">
        {tools.map(({ id, label, icon: Icon, disabled }) => (
          <Button
            key={id}
            disabled={disabled}
            variant="ghost"
            className="h-10 min-w-0 justify-start gap-2.5 border-stroke bg-panel px-3 font-normal text-content max-[650px]:h-11 pointer-coarse:h-11 [&_svg]:text-content-muted"
            onClick={() => onSelect(id)}
          >
            <Icon className="size-4 shrink-0" />
            <span className="truncate">{label}</span>
          </Button>
        ))}
      </div>
    </section>
  )
}
