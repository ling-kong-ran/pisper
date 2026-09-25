// 会话上下文只在用户打开时挂载；文件列表按当前标签请求，不复制聊天状态。
import { useState } from 'react'
import { ExternalLink, Files, Globe2, ListTodo, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { WebPreview, WebPreviewBody } from '@/components/ai-elements/web-preview'
import { normalizeWebPreviewInput } from '@/lib/web-preview'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { Plan } from '@/types/chat'
import PlanBoard from './PlanBoard'
import { SessionFilesPane } from './SessionFilesPane'

export type SessionContextTab = 'files' | 'plan' | 'browser'

type SessionContextPanelProps = {
  panelId: string
  compact: boolean
  embedded?: boolean
  sessionId: string
  tab: SessionContextTab
  plan: Plan | null
  streaming: boolean
  plansAvailable: boolean
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  onTabChange: (tab: SessionContextTab) => void
  onClose: () => void
}

function BrowserPane() {
  const { t } = useI18n()
  const [draft, setDraft] = useState('')
  const [url, setUrl] = useState('')
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
          if (normalized) setUrl(normalized)
        }}
      >
        <input
          aria-label={t('common:webPreview.url')}
          className="min-w-0 flex-1 rounded-md border border-[var(--stroke)] bg-[var(--solid)] px-2 text-[length:var(--app-font-size)]"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder={t('common:webPreview.urlPlaceholder')}
          maxLength={2048}
        />
        <Button type="submit" variant="outline" size="sm">
          {t('chat:sessionContext.openUrl')}
        </Button>
      </form>
      {error && (
        <p
          role="alert"
          className="text-[length:var(--app-small-size)] text-[var(--text-secondary)]"
        >
          {t('chat:sessionContext.invalidUrl')}
        </p>
      )}
      {url ? (
        <WebPreview
          className="min-h-0 flex-1 overflow-hidden rounded-md"
          defaultUrl={url}
          key={url}
        >
          <WebPreviewBody src={url} title={t('common:webPreview.title')} />
        </WebPreview>
      ) : (
        <div className="grid min-h-0 flex-1 place-content-center justify-items-center gap-2 rounded-md border border-dashed border-[var(--stroke-soft)] text-center text-[length:var(--app-font-size)] text-[var(--text-muted)]">
          <Globe2 size={24} aria-hidden="true" />
          <p>{t('chat:sessionContext.browserEmpty')}</p>
        </div>
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
      <p className="text-[length:var(--app-small-size)] text-[var(--text-muted)]">
        {t('common:webPreview.embedNotice')}
      </p>
    </section>
  )
}

export function SessionContextPanel({
  panelId,
  compact,
  embedded = false,
  sessionId,
  tab,
  plan,
  streaming,
  plansAvailable,
  requestConfirm,
  onTabChange,
  onClose,
}: SessionContextPanelProps) {
  const { t } = useI18n()
  const tabs = [
    { id: 'files' as const, icon: Files, label: t('chat:focusSession.fileChanges') },
    ...(plansAvailable
      ? [{ id: 'plan' as const, icon: ListTodo, label: t('chat:sessionContext.plan') }]
      : []),
    { id: 'browser' as const, icon: Globe2, label: t('common:webPreview.title') },
  ]
  const selectedTab = tabs.some((item) => item.id === tab) ? tab : 'files'

  const content = (
    <div
      data-session-context-panel
      className="flex h-full min-h-0 min-w-0 flex-col bg-[var(--panel)] text-[var(--text)]"
    >
      <header className="flex h-12 flex-none items-center justify-between gap-2 border-b border-[var(--stroke-soft)] px-3">
        <strong className="truncate text-[length:var(--app-font-size)] font-semibold">
          {t('chat:sessionContext.title')}
        </strong>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          aria-label={t('chat:sessionContext.close')}
          onClick={onClose}
        >
          <X size={16} />
        </Button>
      </header>
      <div
        className="flex flex-none overflow-x-auto border-b border-[var(--stroke-soft)] px-1"
        role="tablist"
        aria-label={t('chat:sessionContext.title')}
        onKeyDown={(event) => {
          if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
          event.preventDefault()
          const index = tabs.findIndex((item) => item.id === selectedTab)
          const next =
            event.key === 'Home'
              ? 0
              : event.key === 'End'
                ? tabs.length - 1
                : (index + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length
          onTabChange(tabs[next].id)
          const buttons = event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]')
          buttons[next]?.focus()
        }}
      >
        {tabs.map(({ id, icon: Icon, label }) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={selectedTab === id}
            aria-controls={`session-context-${sessionId}-content`}
            tabIndex={selectedTab === id ? 0 : -1}
            className={`flex min-h-11 min-w-11 flex-1 items-center justify-center gap-1.5 border-b-2 px-2 text-[length:var(--app-small-size)] font-medium ${selectedTab === id ? 'border-[var(--brand-blue)] text-[var(--text)]' : 'border-transparent text-[var(--text-secondary)] hover:bg-[var(--surface-hover)]'}`}
            onClick={() => onTabChange(id)}
          >
            <Icon size={14} aria-hidden="true" />
            <span className="whitespace-nowrap">{label}</span>
          </button>
        ))}
      </div>
      <div
        id={`session-context-${sessionId}-content`}
        role="tabpanel"
        className="flex min-h-0 flex-1 flex-col"
        aria-label={tabs.find((item) => item.id === selectedTab)?.label}
      >
        {selectedTab === 'files' ? (
          <SessionFilesPane
            key={sessionId}
            sessionId={sessionId}
            streaming={streaming}
            requestConfirm={requestConfirm}
          />
        ) : selectedTab === 'plan' && plansAvailable ? (
          <div className="min-h-0 flex-1 overflow-y-auto p-3">
            {plan?.items?.length ? (
              <PlanBoard plan={plan} />
            ) : (
              <p className="text-[length:var(--app-font-size)] text-[var(--text-muted)]">
                {t('chat:sessionContext.planEmpty')}
              </p>
            )}
          </div>
        ) : (
          <BrowserPane />
        )}
      </div>
    </div>
  )
  if (!compact) {
    return (
      <aside
        id={panelId}
        tabIndex={embedded ? -1 : undefined}
        className="h-full min-h-0 w-full overflow-hidden rounded-[var(--r-md)] border border-[var(--stroke-soft)]"
        aria-label={t('chat:sessionContext.title')}
      >
        {content}
      </aside>
    )
  }
  return (
    <Sheet open onOpenChange={(open) => !open && onClose()}>
      <SheetContent
        id={panelId}
        side="right"
        showCloseButton={false}
        className="gap-0 overflow-hidden p-0"
        style={{ width: 'min(100vw, 420px)', maxWidth: 'none' }}
      >
        <SheetHeader className="sr-only">
          <SheetTitle>{t('chat:sessionContext.title')}</SheetTitle>
        </SheetHeader>
        {content}
      </SheetContent>
    </Sheet>
  )
}
