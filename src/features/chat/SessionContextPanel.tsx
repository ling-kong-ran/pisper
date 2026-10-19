import { TASK_VIEW_WILL_CHANGE } from './events'
// 会话上下文只在用户打开时挂载；文件列表按当前标签请求，不复制聊天状态。
import { useEffect, useEffectEvent, useMemo, useRef, useState } from 'react'
import {
  ArrowLeft,
  ExternalLink,
  Files,
  FileDiff,
  Link2,
  Globe2,
  ListTodo,
  TerminalSquare,
  Activity,
  MessageSquare,
  Plus,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
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
import { TaskTerminalView, TaskProcessesPane } from '@/features/terminal/public-views'
import { useTerminalService } from '@/features/terminal/public'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { TaskChangesPane } from './TaskChangesPane'
import { TaskSourcesPane } from './TaskSourcesPane'
import type { TaskSource } from './task-sources'
import { TaskNewTab } from './TaskNewTab'
import { SideChatPane } from './SideChatPane'

export type { SessionContextTab } from './session-context-pages'

type SessionContextPanelProps = {
  panelId: string
  compact: boolean
  embedded?: boolean
  active?: boolean
  sessionId: string
  cwd?: string
  sources?: TaskSource[]
  tab: SessionContextTab
  plan: Plan | null
  streaming: boolean
  plansAvailable: boolean
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  onTabChange: (tab: SessionContextTab) => void
  onClose: () => void
  onBack?: () => void
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
  active = true,
  sessionId,
  cwd = '',
  sources = [],
  tab,
  plan,
  streaming,
  plansAvailable,
  requestConfirm,
  onTabChange,
  onClose,
  onBack,
}: SessionContextPanelProps) {
  const { t } = useI18n()
  const terminal = useTerminalService()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const tabs = [
    { id: 'terminal' as const, icon: TerminalSquare, label: t('terminal:workspace.terminal') },
    { id: 'processes' as const, icon: Activity, label: t('terminal:workspace.processes') },
    { id: 'changes' as const, icon: FileDiff, label: t('chat:workbench.projectChanges') },
    { id: 'sources' as const, icon: Link2, label: t('chat:workbench.sources') },
    { id: 'files' as const, icon: Files, label: t('chat:workbench.taskSnapshots') },
    ...(plansAvailable
      ? [{ id: 'plan' as const, icon: ListTodo, label: t('chat:sessionContext.plan') }]
      : []),
    { id: 'browser' as const, icon: Globe2, label: t('common:webPreview.title') },
    { id: 'side-chat' as const, icon: MessageSquare, label: t('chat:sideChat.title') },
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
  const getType = (page: ContextPage) =>
    page.kind === 'new'
      ? { id: 'new' as const, icon: Plus, label: t('chat:workbench.newTab') }
      : (tabs.find((item) => item.id === page.kind) ?? tabs[0])
  const focusTab = (id: string) =>
    requestAnimationFrame(() => document.getElementById(`${panelId}-${id}-tab`)?.focus())
  const focusActiveTab = useEffectEvent(() => {
    // 后台任务仍可打开资源，但不能夺取其他分屏的输入焦点。
    if (active) document.getElementById(`${panelId}-${pages.activeId}-tab`)?.focus()
  })
  useEffect(() => {
    const frame = requestAnimationFrame(() => focusActiveTab())
    return () => cancelAnimationFrame(frame)
  }, [panelId, pages.activeId])
  const dispatch = (action: ContextPageAction) => {
    if (action.type === 'view' && !useContextPagesStore.getState().sessions[sessionId]) return
    if (['add', 'open', 'select', 'close', 'kind'].includes(action.type))
      window.dispatchEvent(new Event(TASK_VIEW_WILL_CHANGE))
    const next = updateContextPages(pages, action)
    useContextPagesStore.getState().update(sessionId, tab, action)
    const active = next.pages.find((page) => page.id === next.activeId)
    if (active) onTabChange(active.kind)
    if (action.type === 'add' || action.type === 'select' || action.type === 'close')
      focusTab(next.activeId)
    if (action.type === 'close' && pages.pages.length === 1) onClose()
  }
  const addPageLabel = t('chat:sessionContext.addPage')
  const pageLimitLabel = t('chat:sessionContext.pageLimit')
  // 页头只保留一个“新增页面”入口：点 + 选种类开新页；单页关闭走标签上的 ×，
  // 整个面板的开关由主区“打开/关闭会话上下文”按钮承担。
  const addMenu = (
    <Button
      variant="ghost"
      size="icon"
      disabled={pages.pages.length >= MAX_CONTEXT_PAGES}
      aria-label={addPageLabel}
      title={pages.pages.length >= MAX_CONTEXT_PAGES ? pageLimitLabel : addPageLabel}
      onClick={() => dispatch({ type: 'add', kind: 'new' })}
    >
      <Plus size={15} />
    </Button>
  )
  const content = (
    <div
      data-session-context-panel
      className="flex h-full min-h-0 min-w-0 flex-col bg-background text-foreground"
    >
      <header
        data-window-drag-region
        className="auxiliary-page-header flex h-14 shrink-0 items-center gap-0.5 border-b border-border/60 px-1.5"
      >
        {onBack && (
          <Button
            variant="ghost"
            size="icon"
            aria-label={t('chat:workbench.backToConversation')}
            title={t('chat:workbench.backToConversation')}
            onClick={onBack}
          >
            <ArrowLeft size={16} />
          </Button>
        )}
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
        {addMenu}
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
            {type.id === 'new' ? (
              active && (
                <TaskNewTab
                  tools={tabs.map((tool) => ({
                    ...tool,
                    disabled:
                      tool.id === 'terminal' || tool.id === 'processes'
                        ? !terminal || !runtimeFeatureAvailable(capabilities, 'terminal')
                        : tool.id === 'changes'
                          ? !runtimeFeatureAvailable(capabilities, 'vcs')
                          : false,
                  }))}
                  onOpenUrl={(url) => {
                    dispatch({ type: 'open', kind: 'browser', replaceId: page.id })
                    useContextPagesStore.getState().update(sessionId, 'browser', {
                      type: 'browser',
                      id: page.id,
                      url,
                      draft: url,
                    })
                  }}
                  onSelect={(kind) => dispatch({ type: 'open', kind, replaceId: page.id })}
                />
              )
            ) : type.id === 'changes' ? (
              active && (
                <TaskChangesPane
                  sessionId={sessionId}
                  cwd={cwd}
                  streaming={streaming}
                  view={page.view}
                  onViewChange={(patch) => dispatch({ type: 'view', id: page.id, patch })}
                />
              )
            ) : type.id === 'sources' ? (
              active && (
                <TaskSourcesPane
                  sources={sources}
                  view={page.view}
                  onViewChange={(patch) => dispatch({ type: 'view', id: page.id, patch })}
                />
              )
            ) : type.id === 'terminal' ? (
              active && (
                <TaskTerminalView
                  sessionId={sessionId}
                  cwd={cwd}
                  terminalId={page.resourceId || undefined}
                  onCreated={(id) =>
                    useContextPagesStore
                      .getState()
                      .update(sessionId, tab, { type: 'resource', id: page.id, resourceId: id })
                  }
                />
              )
            ) : type.id === 'processes' ? (
              active && (
                <TaskProcessesPane
                  view={page.view}
                  onViewChange={(patch) => dispatch({ type: 'view', id: page.id, patch })}
                  sessionId={sessionId}
                  onOpenTerminal={(id) =>
                    dispatch({ type: 'open', kind: 'terminal', resourceId: id })
                  }
                />
              )
            ) : type.id === 'side-chat' ? (
              <SideChatPane parentId={sessionId} active={active} />
            ) : type.id === 'browser' ? (
              <BrowserPane
                page={page}
                onChange={(patch) => dispatch({ type: 'browser', id: page.id, ...patch })}
              />
            ) : (
              active &&
              (type.id === 'files' ? (
                <SessionFilesPane
                  view={page.view}
                  onViewChange={(patch) => dispatch({ type: 'view', id: page.id, patch })}
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
        style={{
          top: 'var(--pisper-safe-area-top)',
          right: 'var(--pisper-safe-area-right)',
          bottom: 'var(--pisper-safe-area-bottom)',
          height: 'auto',
          width:
            'min(calc(100vw - var(--pisper-safe-area-left) - var(--pisper-safe-area-right)), 420px)',
          maxWidth: 'none',
        }}
      >
        <SheetHeader className="sr-only">
          <SheetTitle>{t('chat:sessionContext.title')}</SheetTitle>
        </SheetHeader>
        {content}
      </SheetContent>
    </Sheet>
  )
}
