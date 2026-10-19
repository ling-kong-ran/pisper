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
    <section className="@container/new-tab flex min-h-0 flex-1 flex-col overflow-y-auto bg-surface-subtle/40 p-5">
      <h2 className="mb-4 text-sm font-medium">{t('chat:workbench.newTab')}</h2>
      <form
        className="mb-5 flex gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          const normalized = normalizeWebPreviewInput(url, window.location.href)
          setInvalid(!normalized)
          if (normalized) onOpenUrl(normalized)
        }}
      >
        <Input
          className="h-10 rounded-full bg-background px-4"
          aria-label={t('common:webPreview.url')}
          placeholder={t('common:webPreview.urlPlaceholder')}
          value={url}
          maxLength={2048}
          onChange={(event) => setUrl(event.target.value)}
        />
        <Button type="submit" variant="outline">
          {t('chat:sessionContext.openUrl')}
        </Button>
      </form>
      {invalid && (
        <p role="alert" className="mb-3 text-sm text-destructive">
          {t('chat:sessionContext.invalidUrl')}
        </p>
      )}
      <div className="grid grid-cols-1 gap-2 @min-[420px]/new-tab:grid-cols-2">
        {tools.map(({ id, label, icon: Icon, disabled }) => (
          <Button
            key={id}
            disabled={disabled}
            variant="outline"
            className="h-12 min-w-0 justify-start gap-3 rounded-xl border-transparent bg-background px-3 font-normal shadow-none"
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
