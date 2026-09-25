// 会话上下文只在用户打开时挂载；文件列表按当前标签请求，不复制聊天状态。
import { useMemo, useRef, useState } from 'react'
import { ChevronDown, ExternalLink, Files, Globe2, ListTodo, Plus, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  createContextPages,
  updateContextPages,
  MAX_CONTEXT_PAGES,
  type ContextPage,
  type ContextPageAction,
  type SessionContextTab,
} from './session-context-pages'
import { useContextPagesStore } from './session-context-pages-store'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { WebPreview, WebPreviewBody } from '@/components/ai-elements/web-preview'
import { normalizeWebPreviewInput } from '@/lib/web-preview'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { Plan } from '@/types/chat'
import PlanBoard from './PlanBoard'
import { SessionFilesPane } from './SessionFilesPane'

export type { SessionContextTab } from './session-context-pages'

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

function BrowserPane({
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
        <input
          aria-label={t('common:webPreview.url')}
          className="min-w-0 flex-1 rounded-md border border-[var(--stroke)] bg-[var(--solid)] px-2 text-[length:var(--app-font-size)]"
          value={draft}
          onChange={(event) => onChange({ draft: event.target.value })}
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
  const initial = useMemo(() => createContextPages(tab), [tab])
  const stored = useContextPagesStore((store) =>
    Object.prototype.hasOwnProperty.call(store.sessions, sessionId)
      ? store.sessions[sessionId]
      : undefined,
  )
  const pages = stored ?? initial
  const listRef = useRef<HTMLDivElement>(null)
  const selected = pages.pages.find((page) => page.id === pages.activeId) ?? pages.pages[0]
  const getType = (page: ContextPage) => tabs.find((item) => item.id === page.kind) ?? tabs[0]
  const focusTab = (id: string) =>
    requestAnimationFrame(() => document.getElementById(`${panelId}-${id}-tab`)?.focus())
  const dispatch = (action: ContextPageAction) => {
    const next = updateContextPages(pages, action)
    useContextPagesStore.getState().update(sessionId, tab, action)
    const active = next.pages.find((page) => page.id === next.activeId)
    if (active) onTabChange(active.kind)
    if (action.type === 'add' || action.type === 'select' || action.type === 'close')
      focusTab(next.activeId)
    if (action.type === 'close' && pages.pages.length === 1) onClose()
  }
  const addPageLabel = t('chat:sessionContext.addPage')
  const pageContentLabel = t('chat:sessionContext.pageContent')
  const pageLimitLabel = t('chat:sessionContext.pageLimit')
  const contentMenu = (add: boolean) => (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-muted disabled:opacity-40"
          disabled={add && pages.pages.length >= MAX_CONTEXT_PAGES}
          aria-label={add ? addPageLabel : pageContentLabel}
          title={
            add && pages.pages.length >= MAX_CONTEXT_PAGES
              ? pageLimitLabel
              : add
                ? addPageLabel
                : pageContentLabel
          }
        >
          {add ? <Plus size={15} /> : <ChevronDown size={14} />}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-40">
        {tabs.map(({ id, icon: Icon, label }) => (
          <DropdownMenuItem
            key={id}
            onSelect={() =>
              dispatch(
                add ? { type: 'add', kind: id } : { type: 'kind', id: selected.id, kind: id },
              )
            }
          >
            <Icon size={14} />
            {label}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
  const content = (
    <div
      data-session-context-panel
      className="flex h-full min-h-0 min-w-0 flex-col bg-background text-foreground"
    >
      <header
        data-window-drag-region
        className="auxiliary-page-header flex h-12 shrink-0 items-center gap-0.5 border-b border-border/60 px-1.5"
      >
        <div
          ref={listRef}
          className="flex min-w-0 flex-1 items-center overflow-x-auto"
          role="tablist"
          aria-label={t('chat:sessionContext.title')}
          onKeyDown={(event) => {
            if (
              !(event.target instanceof HTMLElement) ||
              event.target.getAttribute('role') !== 'tab'
            )
              return
            if (event.key === 'Delete') {
              event.preventDefault()
              dispatch({ type: 'close', id: selected.id })
              return
            }
            if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
            event.preventDefault()
            const index = pages.pages.findIndex((page) => page.id === selected.id)
            const next =
              event.key === 'Home'
                ? 0
                : event.key === 'End'
                  ? pages.pages.length - 1
                  : (index + (event.key === 'ArrowRight' ? 1 : -1) + pages.pages.length) %
                    pages.pages.length
            dispatch({ type: 'select', id: pages.pages[next].id })
          }}
        >
          {pages.pages.map((page) => {
            const { icon: Icon, label } = getType(page)
            return (
              <div
                key={page.id}
                className={`group flex max-w-40 min-w-20 shrink-0 items-center rounded-md ${page.id === selected.id ? 'bg-muted' : 'text-muted-foreground'}`}
              >
                <button
                  id={`${panelId}-${page.id}-tab`}
                  type="button"
                  role="tab"
                  aria-selected={page.id === selected.id}
                  aria-controls={`${panelId}-${page.id}-content`}
                  tabIndex={page.id === selected.id ? 0 : -1}
                  className="flex h-8 min-w-0 flex-1 items-center gap-1.5 px-2 text-xs"
                  onClick={() => dispatch({ type: 'select', id: page.id })}
                >
                  <Icon size={13} className="shrink-0" />
                  <span className="truncate">{label}</span>
                </button>
                <button
                  type="button"
                  className="mr-0.5 grid size-5 shrink-0 place-items-center rounded hover:bg-foreground/10"
                  aria-label={`${t('chat:sessionContext.closePage')} · ${label}`}
                  onClick={() => dispatch({ type: 'close', id: page.id })}
                >
                  <X size={11} />
                </button>
              </div>
            )
          })}
        </div>
        {contentMenu(true)}
        {contentMenu(false)}
        <button
          type="button"
          className="grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-muted"
          aria-label={t('chat:sessionContext.close')}
          onClick={onClose}
        >
          <X size={15} />
        </button>
      </header>
      {pages.pages.map((page) => {
        const type = getType(page)
        const active = selected.id === page.id
        return (
          <div
            key={page.id}
            id={`${panelId}-${page.id}-content`}
            role="tabpanel"
            aria-labelledby={`${panelId}-${page.id}-tab`}
            className={active ? 'flex min-h-0 flex-1 flex-col' : 'hidden'}
            hidden={!active}
          >
            {type.id === 'browser' ? (
              <BrowserPane
                page={page}
                onChange={(patch) => dispatch({ type: 'browser', id: page.id, ...patch })}
              />
            ) : (
              active &&
              (type.id === 'files' ? (
                <SessionFilesPane
                  sessionId={sessionId}
                  streaming={streaming}
                  requestConfirm={requestConfirm}
                />
              ) : (
                <div className="min-h-0 flex-1 overflow-y-auto p-3">
                  {plan?.items?.length ? (
                    <PlanBoard plan={plan} />
                  ) : (
                    <p className="text-sm text-muted-foreground">
                      {t('chat:sessionContext.planEmpty')}
                    </p>
                  )}
                </div>
              ))
            )}
          </div>
        )
      })}
    </div>
  )
  if (!compact) {
    return (
      <aside
        id={panelId}
        tabIndex={embedded ? -1 : undefined}
        className="h-full min-h-0 w-full overflow-hidden border-l border-border/60"
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
