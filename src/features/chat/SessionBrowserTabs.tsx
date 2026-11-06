// 检查器“预览”里的浏览器页签：每个页签一个 BrowserPane，切走时只隐藏，网页不重新加载。
// 没看过的页签不挂载，不会一开检查器就加载所有网页。
import { useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Plus, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { webPreviewTabLabel } from '@/lib/web-preview'
import { cn } from '@/lib/utils'
import { BrowserPane } from './SessionBrowserPane'
import { activeBrowserPage, MAX_CONTEXT_PAGES } from './session-context-pages'
import { useContextPagesStore } from './session-context-pages-store'

export function BrowserTabs({ panelId, sessionId }: { panelId: string; sessionId: string }) {
  const { t } = useI18n()
  const state = useContextPagesStore((store) => store.sessions[sessionId])
  const allPages = state?.pages
  const browsers = allPages?.filter((page) => page.kind === 'browser') ?? []
  const active = state && activeBrowserPage(state)
  const activeId = active?.id ?? ''
  const atLimit = (allPages?.length ?? 0) >= MAX_CONTEXT_PAGES
  const listRef = useRef<HTMLDivElement>(null)

  const [visited, setVisited] = useState<string[]>([])
  if (activeId && !visited.includes(activeId)) setVisited([...visited, activeId])

  useEffect(() => {
    // 滚动整个页签而不是里面的按钮，否则右边的关闭键还在可视区外。
    listRef.current
      ?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')
      ?.closest('[data-browser-tab]')
      ?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' })
  }, [activeId])

  const tabId = (id: string) => `${panelId}-browser-${id}-tab`
  const panelIdOf = (id: string) => `${panelId}-browser-${id}-panel`
  const select = (id: string) =>
    useContextPagesStore.getState().update(sessionId, 'browser', { type: 'browser-tab', id })
  const focusAfterRender = (query: (id: string) => string, id: string, selector = '') =>
    requestAnimationFrame(() =>
      document.querySelector<HTMLElement>(`[id="${query(id)}"] ${selector}`.trim())?.focus(),
    )

  const addTab = () => {
    const store = useContextPagesStore.getState()
    const contentOpen = store.sessions[sessionId]?.contentOpen ?? false
    if (store.update(sessionId, 'browser', { type: 'add', kind: 'browser' }) === 'limit') return
    // 和检查器按需建页一样，不改手机外壳“只显示内容”的开关。
    if (!contentOpen) store.update(sessionId, 'browser', { type: 'workspace', contentOpen: false })
    const next = useContextPagesStore.getState().sessions[sessionId]
    const created = next && activeBrowserPage(next)
    if (created) focusAfterRender(panelIdOf, created.id, 'input')
  }

  const closeTab = (id: string) => {
    const store = useContextPagesStore.getState()
    // 最后一个页签关掉后变回空白页，留在“预览”里，也还能输入地址。
    if (browsers.length === 1) {
      store.update(sessionId, 'browser', { type: 'browser', id, url: '', draft: '' })
      store.update(sessionId, 'browser', { type: 'resource', id, resourceId: '' })
      return
    }
    store.update(sessionId, 'browser', { type: 'close', id })
    const next = useContextPagesStore.getState().sessions[sessionId]
    const nextActive = next && activeBrowserPage(next)
    if (nextActive) focusAfterRender(tabId, nextActive.id)
  }

  const tabKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!(event.target instanceof HTMLElement) || event.target.getAttribute('role') !== 'tab')
      return
    if (event.key === 'Delete') {
      event.preventDefault()
      if (activeId) closeTab(activeId)
      return
    }
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
    event.preventDefault()
    const index = browsers.findIndex((page) => page.id === activeId)
    const nextIndex =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? browsers.length - 1
          : (index + (event.key === 'ArrowRight' ? 1 : -1) + browsers.length) % browsers.length
    const next = browsers[nextIndex]
    if (!next) return
    select(next.id)
    focusAfterRender(tabId, next.id)
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-1 border-b border-stroke-soft px-3 py-1.5">
        <div
          ref={listRef}
          role="tablist"
          aria-label={t('common:webPreview.tabs')}
          className="flex min-w-0 items-center gap-0.5 overflow-x-auto [scrollbar-width:none]"
          onKeyDown={tabKeyDown}
        >
          {browsers.map((page) => {
            const selected = page.id === activeId
            const label =
              page.title ||
              (page.url ? webPreviewTabLabel(page.url) : t('common:webPreview.newTab'))
            // 只剩一个空白页签时没什么可关的。
            const closable = browsers.length > 1 || Boolean(page.url || page.resourceId)
            return (
              <div
                key={page.id}
                data-browser-tab={page.id}
                className={cn(
                  // 和浏览器一样先收窄，收到最窄还放不下才横向滚动。
                  'flex min-w-20 flex-[0_1_10rem] items-center rounded-sm text-[13px] transition-colors duration-(--d-fast)',
                  selected
                    ? 'bg-solid font-medium text-content shadow-edge ring-1 ring-stroke-strong ring-inset'
                    : 'text-content-secondary hover:bg-surface-hover hover:text-content',
                )}
                onAuxClick={(event) => {
                  if (event.button === 1 && closable) closeTab(page.id)
                }}
              >
                <button
                  id={tabId(page.id)}
                  type="button"
                  role="tab"
                  aria-selected={selected}
                  aria-controls={panelIdOf(page.id)}
                  tabIndex={selected ? 0 : -1}
                  title={page.url || label}
                  className={cn(
                    'flex h-7 min-w-0 flex-1 items-center rounded-sm pl-2 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11',
                    !closable && 'pr-2',
                  )}
                  onClick={() => select(page.id)}
                >
                  <span className="truncate">{label}</span>
                </button>
                {closable && (
                  <button
                    type="button"
                    tabIndex={selected ? 0 : -1}
                    aria-label={`${t('common:webPreview.closeTab')} · ${label}`}
                    className="mx-0.5 grid size-5 flex-none place-items-center rounded-xs text-content-muted hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:size-11"
                    onClick={() => closeTab(page.id)}
                  >
                    <X size={12} aria-hidden="true" />
                  </button>
                )}
              </div>
            )
          })}
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="flex-none pointer-coarse:size-11"
          disabled={atLimit}
          aria-label={t('common:webPreview.newTab')}
          title={atLimit ? t('chat:sessionContext.pageLimit') : t('common:webPreview.newTab')}
          onClick={addTab}
        >
          <Plus aria-hidden="true" />
        </Button>
      </div>
      {browsers
        .filter((page) => visited.includes(page.id))
        .map((page) => {
          const selected = page.id === activeId
          return (
            <div
              key={page.id}
              id={panelIdOf(page.id)}
              role="tabpanel"
              aria-labelledby={tabId(page.id)}
              hidden={!selected}
              className={selected ? 'flex min-h-0 flex-1 flex-col' : 'hidden'}
            >
              <BrowserPane
                sessionId={sessionId}
                page={page}
                onChange={(patch) =>
                  useContextPagesStore
                    .getState()
                    .update(sessionId, 'browser', { type: 'browser', id: page.id, ...patch })
                }
                onClosed={() => closeTab(page.id)}
              />
            </div>
          )
        })}
    </div>
  )
}
