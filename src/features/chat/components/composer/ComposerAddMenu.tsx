// 桌面“+”菜单：列出真实可调用资源；选择只更新草稿，发送行为仍由 Composer 持有。
import { useEffect, useRef, useState, type ReactNode, type RefObject } from 'react'
import { Braces, FileText, Minimize2, Search, Target, Workflow, Wrench } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { chatApi } from '@/features/chat/api/chat-api'
import { getSessionPluginCatalog, toolDescription, toolName } from '@/features/plugins/public'
import type { ResourceInvocation } from '@/types/chat'
import { AnchoredPopupMenu } from '@/features/chat/components/AnchoredPopupMenu'

type Entry = {
  id: string
  kind: 'tool' | 'skill' | 'prompt'
  name: string
  description: string
  invocation?: string
}

type Category = 'context' | 'capability' | 'skill'
// 只给已在当前会话可调用的能力排序，不据此开放或虚构能力。
const COMMON_TOOLS = ['read', 'edit', 'write', 'bash', 'web_search', 'grep', 'find', 'ls']

export function ComposerAddMenu({
  open,
  sessionId,
  anchorRef,
  menuRef,
  menuId,
  attachment,
  directory,
  visual,
  settings,
  goalsAvailable,
  workflowsAvailable,
  goalDisabled,
  onGoal,
  onWorkflow,
  onClose,
  onSelect,
  onCommandSelect,
}: {
  open: boolean
  sessionId: string
  anchorRef: RefObject<HTMLElement | null>
  menuRef: RefObject<HTMLDivElement | null>
  menuId: string
  attachment: ReactNode
  directory?: ReactNode
  visual?: ReactNode
  settings?: ReactNode
  goalsAvailable: boolean
  workflowsAvailable: boolean
  goalDisabled: boolean
  onGoal: () => void
  onWorkflow: () => void
  onClose: () => void
  onSelect: (value: ResourceInvocation) => void
  onCommandSelect: (command: string) => void
}) {
  const { t } = useI18n()
  const [entries, setEntries] = useState<Entry[]>([])
  const [query, setQuery] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [category, setCategory] = useState<Category | null>(null)
  const [expanded, setExpanded] = useState(false)
  const searchRef = useRef<HTMLInputElement>(null)
  const composingRef = useRef(false)
  const entriesRef = useRef<HTMLDivElement>(null)
  // 列表键盘导航仅处理本菜单内的搜索/候选，不吞掉嵌套设置面板的按键。
  useEffect(() => {
    if (!open) return
    const navigate = (event: KeyboardEvent) => {
      if (event.isComposing || composingRef.current || event.keyCode === 229) return
      if (event.defaultPrevented || !['ArrowDown', 'ArrowUp', 'Enter'].includes(event.key)) return
      const buttons = [
        ...(entriesRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') || []),
      ].filter((button) => button.getClientRects().length > 0)
      const target = document.activeElement
      const index = buttons.findIndex((button) => button === target)
      const searching = target === searchRef.current
      if ((!searching && index < 0) || !buttons.length) return
      if (event.key === 'Enter') {
        if (!searching) return
        event.preventDefault()
        buttons[0].click()
        return
      }
      event.preventDefault()
      const next = searching
        ? event.key === 'ArrowDown'
          ? 0
          : buttons.length - 1
        : (index + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length
      buttons[next].focus()
      buttons[next].scrollIntoView({ block: 'nearest' })
    }
    document.addEventListener('keydown', navigate)
    return () => document.removeEventListener('keydown', navigate)
  }, [open])
  useEffect(() => {
    if (!open) return
    let active = true
    const controller = new AbortController()
    searchRef.current?.focus()
    composingRef.current = false
    setQuery('')
    setCategory(null)
    setExpanded(false)
    setEntries([])
    setLoading(true)
    setError('')
    void Promise.allSettled([
      chatApi.getSessionCommands(sessionId, controller.signal),
      getSessionPluginCatalog(sessionId, controller.signal),
    ])
      .then(([commandResult, pluginResult]) => {
        if (!active) return
        const commands = commandResult.status === 'fulfilled' ? commandResult.value : null
        const plugins = pluginResult.status === 'fulfilled' ? pluginResult.value : null
        if (commandResult.status === 'rejected' || pluginResult.status === 'rejected')
          setError(t('chat:addMenu.loadError'))
        const callable = new Set(plugins?.callableToolNames || [])
        setEntries([
          ...(plugins?.tools || [])
            .filter((capability) => callable.has(capability.name))
            .map<Entry>((capability) => ({
              kind: 'tool',
              id: capability.name,
              name: capability.id
                ? toolName(capability, t)
                : String(capability.label || capability.name),
              description: capability.id ? toolDescription(capability, t) : capability.description,
            })),
          ...(commands?.commands || [])
            .filter((command) => command.invocation !== '/compact')
            .map<Entry>((command) => ({
              id: command.name,
              kind: command.source,
              name: command.name,
              description: command.description || '',
              invocation: command.invocation,
            })),
        ])
      })
      .catch((caught) => {
        if (active) setError(caught instanceof Error ? caught.message : String(caught))
      })
      .finally(() => {
        if (active) setLoading(false)
      })
    return () => {
      active = false
      controller.abort()
    }
  }, [open, sessionId, t])
  const matches = (text: string) =>
    text.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())
  const visible = entries.filter((entry) =>
    `${entry.id} ${entry.name} ${entry.description}`
      .toLocaleLowerCase()
      .includes(query.trim().toLocaleLowerCase()),
  )
  const capabilities = visible
    .filter((entry) => entry.kind !== 'skill')
    .sort((a, b) => {
      const rank = (entry: Entry) => {
        const position = entry.kind === 'tool' ? COMMON_TOOLS.indexOf(entry.id) : -1
        return position < 0 ? COMMON_TOOLS.length : position
      }
      return rank(a) - rank(b)
    })
  const skills = visible.filter((entry) => entry.kind === 'skill')
  const preview = (items: Entry[]) =>
    query.trim() || expanded || category ? items : items.slice(0, 4)
  const show = (kind: Category) => !category || category === kind
  const changeCategory = (next: Category) => {
    setCategory((current) => (current === next ? null : next))
    setQuery('')
    setExpanded(false)
    searchRef.current?.focus()
  }
  const choose = (entry: Entry) => {
    if (entry.kind === 'prompt') onCommandSelect(entry.invocation || `/${entry.name}`)
    else
      onSelect({
        kind: entry.kind,
        resourceId: entry.id,
        resourceName: entry.name,
        behavior: 'foreground',
      })
    onClose()
  }
  const row =
    'flex min-h-10 w-full min-w-0 items-center gap-3 rounded-xl px-3 py-2 text-left text-sm hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:opacity-50'
  const renderEntry = (entry: Entry) => {
    const Icon = entry.kind === 'tool' ? Wrench : entry.kind === 'skill' ? Braces : FileText
    return (
      <button
        type="button"
        key={`${entry.kind}:${entry.id}`}
        className={row}
        onClick={() => choose(entry)}
        aria-label={`${entry.name} ${entry.description}`}
        title={entry.description}
      >
        <span className="grid size-6 shrink-0 place-items-center rounded-md bg-muted">
          <Icon size={15} />
        </span>
        <span className="max-w-[45%] shrink-0 truncate">{entry.name}</span>
        <span className="min-w-0 truncate text-xs text-muted-foreground">{entry.description}</span>
      </button>
    )
  }
  const heading = 'px-3 pb-1 pt-3 text-xs font-semibold text-muted-foreground'
  return (
    <AnchoredPopupMenu
      open={open}
      anchorRef={anchorRef}
      menuRef={menuRef}
      placement="top"
      matchAnchorWidth
      maxHeight={480}
      id={menuId}
      role="dialog"
      ariaLabel={t('chat:addMenu.title')}
      className="composer-tool-tray-shell composer-add-menu flex flex-col overflow-hidden rounded-2xl border border-border bg-popover p-1.5 text-popover-foreground shadow-xl"
    >
      <div className="composer-tool-tray min-h-0 overflow-y-auto overscroll-contain">
        <div ref={entriesRef}>
          {show('context') && (
            <section aria-label={t('chat:addMenu.context')}>
              <p className={heading}>{t('chat:addMenu.context')}</p>
              {matches(t('chat:addMenu.contextSearch')) && (
                <>
                  <div className="[&_.attach-trigger]:min-h-10 [&_.attach-trigger]:w-full [&_.attach-trigger]:justify-start [&_.attach-trigger]:gap-3 [&_.attach-trigger]:rounded-xl [&_.attach-trigger]:px-3 [&_.attach-trigger]:text-sm">
                    {attachment}
                    {directory}
                  </div>
                </>
              )}
            </section>
          )}
          {show('capability') && (
            <section aria-label={t('chat:addMenu.capabilities')}>
              <p className={heading}>{t('chat:addMenu.capabilities')}</p>
              {goalsAvailable && matches(t('chat:addMenu.goal')) && (
                <button type="button" className={row} disabled={goalDisabled} onClick={onGoal}>
                  <Target size={17} />
                  {t('chat:addMenu.goal')}
                </button>
              )}
              {workflowsAvailable && matches(t('chat:addMenu.workflow')) && (
                <button type="button" className={row} onClick={onWorkflow}>
                  <Workflow size={17} />
                  {t('chat:addMenu.workflow')}
                </button>
              )}
              {matches(`/compact ${t('chat:commands.compactDescription')}`) && (
                <button
                  type="button"
                  className={row}
                  aria-label={`/compact ${t('chat:commands.compactDescription')}`}
                  onClick={() => {
                    onCommandSelect('/compact')
                    onClose()
                  }}
                >
                  <Minimize2 size={17} />
                  <span>/compact</span>
                  <span className="min-w-0 truncate text-xs text-muted-foreground">
                    {t('chat:commands.compactDescription')}
                  </span>
                </button>
              )}
              {!query.trim() && visual}
              {preview(capabilities).map(renderEntry)}
            </section>
          )}
          {show('skill') && (
            <section aria-label={t('chat:addMenu.skills')}>
              <p className={heading}>{t('chat:addMenu.skills')}</p>
              {preview(skills).map(renderEntry)}
              {!loading && !skills.length && (
                <p className="px-3 py-2 text-xs text-muted-foreground">
                  {query.trim() ? t('chat:addMenu.empty') : t('chat:addMenu.noSkills')}
                </p>
              )}
            </section>
          )}
          {!category &&
            !query.trim() &&
            !expanded &&
            (capabilities.length > 4 || skills.length > 4) && (
              <button type="button" className={row} onClick={() => setExpanded(true)}>
                {t('chat:addMenu.showAll')}
              </button>
            )}
        </div>
        {!category && !query.trim() && settings && (
          <section>
            <p className="px-3 pb-1 pt-3 text-xs font-semibold text-muted-foreground">
              {t('chat:addMenu.settings')}
            </p>
            {settings}
          </section>
        )}
        {loading && (
          <p role="status" className="px-3 py-3 text-xs text-muted-foreground">
            {t('chat:addMenu.loading')}
          </p>
        )}
        {error && (
          <p role="alert" className="px-3 py-3 text-xs text-destructive">
            {error}
          </p>
        )}
      </div>
      <label className="mx-2 mt-2 flex shrink-0 items-center gap-2 border-t border-border pt-2 text-muted-foreground">
        <Search size={14} />
        <input
          ref={searchRef}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onCompositionStart={() => {
            composingRef.current = true
          }}
          onCompositionEnd={() => {
            composingRef.current = false
          }}
          aria-label={t('chat:addMenu.search')}
          placeholder={t('chat:addMenu.search')}
          className="h-8 min-w-0 flex-1 bg-transparent text-xs outline-none"
        />
      </label>
      <div className="flex shrink-0 flex-wrap gap-1 px-1 py-1 text-xs text-muted-foreground">
        <button
          type="button"
          className="rounded-md px-2 py-1.5 hover:bg-accent aria-pressed:bg-accent aria-pressed:text-foreground"
          aria-pressed={category === 'context'}
          onClick={() => changeCategory('context')}
        >
          @ {t('chat:addMenu.context')}
        </button>
        <button
          type="button"
          className="rounded-md px-2 py-1.5 hover:bg-accent aria-pressed:bg-accent aria-pressed:text-foreground"
          aria-pressed={category === 'capability'}
          onClick={() => changeCategory('capability')}
        >
          / {t('chat:addMenu.capabilities')}
        </button>
        <button
          type="button"
          className="rounded-md px-2 py-1.5 hover:bg-accent aria-pressed:bg-accent aria-pressed:text-foreground"
          aria-pressed={category === 'skill'}
          onClick={() => changeCategory('skill')}
        >
          $ {t('chat:addMenu.skills')}
        </button>
      </div>
    </AnchoredPopupMenu>
  )
}
