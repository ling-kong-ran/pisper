// 全站浮层：命令面板（Cmd+K，导航/搜索/直达）与快捷创建（新会话/资源）。
// 两个都懒加载并按需挂载，避免常驻开销；搜索会话走 React Query 缓存。
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
} from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  AlertCircle,
  LoaderCircle,
  MessageSquare,
  Plus,
  Search,
  SunMoon,
  Tag,
  TerminalSquare,
  X,
  type LucideIcon,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { startupQueryOptions } from '@/lib/startup-queries'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Kbd } from '@/components/ui/kbd'
import { FieldLabel } from '@/components/ui/field'
import { relativeTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import { chatApi, type SessionTreeLabelMatch } from '@/features/chat/chat-api'
import { takeCommandPaletteQuery } from '@/features/chat/events'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'

import { AppCardHeader } from '@/components/ui/app-primitives'

type Navigation = Array<[string, Array<[string, string, LucideIcon]>]>

type SessionSummary = {
  id: string
  name?: string
  firstMessage?: string
  modified: string
}

type CommandEntry = {
  id: string
  Icon: LucideIcon
  label: string
  hint: string
  meta?: string
  title?: string
  run: () => void | Promise<unknown>
  group: 'actions' | 'pages' | 'labels' | 'sessions'
  keepOpen?: boolean
}

type CommandPaletteProps = {
  navigation: Navigation
  onClose: () => void
  onNavigate: (page: string) => void
  onOpenSession: (
    sessionId: string,
    targetEntryId?: string,
    targetActive?: boolean,
  ) => Promise<void>
  onNewChat: () => void
  // “切换主题”命令：主题键已从页头去掉（规格 v2 第 2.5 节），⌘K 与设置 › 界面是它的入口。
  onToggleTheme?: () => void
}

export function CommandPalette({
  navigation,
  onClose,
  onNavigate,
  onOpenSession,
  onNewChat,
  onToggleTheme,
}: CommandPaletteProps) {
  const { t, language } = useI18n()
  const listId = useId()
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const composing = useRef(false)
  const pendingRef = useRef(false)
  const pointerPosition = useRef({ x: -1, y: -1 })
  // 从“更多”页搜索框转来时带着关键词打开（与 ⌘K 同一份数据源）；⌘K 直接打开时为空。
  const [query, setQuery] = useState(takeCommandPaletteQuery)
  const keyword = query.trim()
  // 桌面全局终端面板（规格 v2 第 2.5 节）：会话标题菜单不再放终端项，⌘K 与 ⌘` 是它的入口；
  // 只在桌面壳提供终端配置且 Runtime 开了终端能力时出现，执行时交给应用壳已有的切换事件。
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const terminalAvailable =
    Boolean(window.pisperDesktop?.terminalProfiles) &&
    runtimeFeatureAvailable(capabilities, 'terminal')
  const sessionQuery = useQuery(startupQueryOptions<{ sessions?: SessionSummary[] }>('sessions'))
  const { isError: sessionsError, refetch: refetchSessions } = sessionQuery
  const sessions = useMemo(() => sessionQuery.data?.sessions || [], [sessionQuery.data])
  const [labelMatches, setLabelMatches] = useState<SessionTreeLabelMatch[]>([])
  const [labelsSearching, setLabelsSearching] = useState(false)
  const [labelsError, setLabelsError] = useState(false)
  const [retry, setRetry] = useState(0)
  const [activeId, setActiveId] = useState<string | null>(null)
  const [pendingEntryId, setPendingEntryId] = useState('')
  const [openError, setOpenError] = useState('')
  const [pointerSnapshot, setPointerSnapshot] = useState<{
    query: string
    entries: CommandEntry[]
  } | null>(null)

  useEffect(() => {
    setLabelsError(false)
    setLabelMatches([])
    if (!keyword) {
      setLabelsSearching(false)
      return
    }
    const controller = new AbortController()
    setLabelsSearching(true)
    const timer = window.setTimeout(() => {
      void chatApi.searchSessionTreeLabels(keyword, 12, controller.signal).then(
        (data) => {
          if (controller.signal.aborted) return
          setLabelMatches(data.labels || [])
          setLabelsSearching(false)
        },
        () => {
          if (controller.signal.aborted) return
          setLabelsError(true)
          setLabelsSearching(false)
        },
      )
    }, 250)
    return () => {
      controller.abort()
      window.clearTimeout(timer)
    }
  }, [keyword, retry])

  const groups = {
    actions: t('navigation:appOverlays.actions'),
    pages: t('navigation:appOverlays.pages'),
    labels: t('navigation:appOverlays.labels'),
    sessions: t('navigation:appSidebar.recentChats'),
  }
  const currentEntries = useMemo(() => {
    const needle = keyword.toLocaleLowerCase(language)
    const matches = (...values: unknown[]) =>
      !needle ||
      values.some((value) =>
        String(value || '')
          .toLocaleLowerCase(language)
          .includes(needle),
      )
    const result: CommandEntry[] = []
    const newChatLabel = t('navigation:appOverlays.newChat')
    if (matches(newChatLabel, t('navigation:appOverlays.actions'))) {
      result.push({
        id: 'action:new-chat',
        group: 'actions',
        Icon: Plus,
        label: newChatLabel,
        hint: t('navigation:appOverlays.actions'),
        run: onNewChat,
      })
    }
    const themeLabel = t('navigation:appOverlays.toggleTheme')
    if (onToggleTheme && matches(themeLabel, t('navigation:appOverlays.actions'), 'theme')) {
      result.push({
        id: 'action:toggle-theme',
        group: 'actions',
        Icon: SunMoon,
        label: themeLabel,
        hint: t('navigation:appOverlays.actions'),
        run: onToggleTheme,
      })
    }
    const terminalLabel = t('navigation:pageHeader.toggleTerminal')
    if (
      terminalAvailable &&
      matches(terminalLabel, t('navigation:appOverlays.actions'), 'terminal')
    )
      result.push({
        id: 'action:toggle-terminal',
        group: 'actions',
        Icon: TerminalSquare,
        label: terminalLabel,
        hint: t('navigation:appOverlays.actions'),
        run: () => {
          window.dispatchEvent(new Event('pisper:toggle-terminal'))
        },
      })
    for (const [group, items] of navigation) {
      for (const [id, label, Icon] of items) {
        if (matches(label, group, id))
          result.push({
            id: `page:${id}`,
            group: 'pages',
            Icon,
            label,
            hint: group,
            run: () => onNavigate(id),
          })
      }
    }
    const formatTimestamp = (value: string) => {
      const timestamp = Date.parse(value)
      if (!Number.isFinite(timestamp)) return t('navigation:appOverlays.unknownTime')
      return new Intl.DateTimeFormat(language, { dateStyle: 'medium', timeStyle: 'short' }).format(
        timestamp,
      )
    }
    for (const label of labelMatches) {
      const sessionTime = formatTimestamp(label.sessionCreated || label.sessionModified)
      const nodeTime = formatTimestamp(label.nodeTimestamp)
      result.push({
        id: `label:${label.sessionId}:${label.entryId}`,
        group: 'labels',
        Icon: Tag,
        label: label.label,
        hint: label.sessionName || t('navigation:appOverlays.untitledChat'),
        meta: t('navigation:appOverlays.labelResultTimes', { sessionTime, nodeTime }),
        title: label.summary || label.label,
        run: () => onOpenSession(label.sessionId, label.entryId, label.active),
      })
    }
    if (labelsError)
      result.push({
        id: 'retry:labels',
        group: 'labels',
        Icon: AlertCircle,
        label: t('navigation:appOverlays.searchLabelsFailed'),
        hint: t('common:pageTemplate.retry'),
        run: () => setRetry((value) => value + 1),
        keepOpen: true,
      })
    let sessionCount = 0
    for (const session of [...sessions].sort(
      (a, b) => Date.parse(b.modified) - Date.parse(a.modified),
    )) {
      if (!matches(session.name, session.firstMessage)) continue
      result.push({
        id: `session:${session.id}`,
        group: 'sessions',
        Icon: MessageSquare,
        label: session.name || t('navigation:appOverlays.untitledChat'),
        hint: relativeTime(session.modified, language),
        run: () => onOpenSession(session.id),
      })
      if (++sessionCount >= 8) break
    }
    if (sessionsError)
      result.push({
        id: 'retry:sessions',
        group: 'sessions',
        Icon: AlertCircle,
        label: t('navigation:appOverlays.loadChatsFailed'),
        hint: t('common:pageTemplate.retry'),
        run: () => refetchSessions(),
        keepOpen: true,
      })
    return result
  }, [
    keyword,
    language,
    labelMatches,
    labelsError,
    navigation,
    onNavigate,
    onNewChat,
    onOpenSession,
    onToggleTheme,
    sessions,
    sessionsError,
    refetchSessions,
    t,
    terminalAvailable,
  ])

  // 同一查询下冻结指针进入时的结果，防止迟到的标签插入后改变点击对象。
  const entries = pointerSnapshot?.query === query ? pointerSnapshot.entries : currentEntries
  const selectedIndex = Math.max(
    0,
    entries.findIndex((entry) => entry.id === activeId),
  )
  const selectedEntry = entries[selectedIndex]
  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' })
  }, [activeId])

  const runEntry = async (entry?: CommandEntry) => {
    if (!entry || pendingRef.current) return
    pendingRef.current = true
    setPointerSnapshot(null)
    setPendingEntryId(entry.id)
    setOpenError('')
    try {
      await entry.run()
      if (!entry.keepOpen) onClose()
    } catch {
      setOpenError(
        entry.group === 'labels'
          ? t('navigation:appOverlays.openLabeledTurnFailed')
          : t('navigation:appOverlays.openResultFailed'),
      )
    } finally {
      pendingRef.current = false
      setPendingEntryId('')
      inputRef.current?.focus()
    }
  }
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing || composing.current || event.keyCode === 229) return
    if (pendingRef.current) {
      event.preventDefault()
      return
    }
    const select = (index: number) => {
      setPointerSnapshot(null)
      setActiveId(entries[index]?.id ?? null)
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      const direction = event.key === 'ArrowDown' ? 1 : -1
      select(entries.length ? (selectedIndex + direction + entries.length) % entries.length : 0)
    } else if (event.key === 'Tab' && !event.altKey && !event.ctrlKey && !event.metaKey) {
      const starts = entries.flatMap((entry, index) =>
        index === 0 || entry.group !== entries[index - 1].group ? [index] : [],
      )
      if (starts.length < 2) return
      event.preventDefault()
      const group = starts.findIndex((start) => entries[start].group === selectedEntry?.group)
      select(starts[(group + (event.shiftKey ? -1 : 1) + starts.length) % starts.length])
    } else if (event.key === 'Enter') {
      event.preventDefault()
      void runEntry(selectedEntry)
    }
  }
  const highlight = (label: string) => {
    const start = keyword
      ? label.toLocaleLowerCase(language).indexOf(keyword.toLocaleLowerCase(language))
      : -1
    if (start < 0) return label
    return (
      <>
        {label.slice(0, start)}
        <mark className="bg-transparent font-semibold text-foreground">
          {label.slice(start, start + keyword.length)}
        </mark>
        {label.slice(start + keyword.length)}
      </>
    )
  }
  const searching = labelsSearching || sessionQuery.isPending
  return (
    <Dialog open onOpenChange={(open) => !open && !pendingRef.current && onClose()}>
      <DialogContent
        showCloseButton={false}
        aria-busy={Boolean(pendingEntryId)}
        onEscapeKeyDown={(event) => {
          if (composing.current || pendingRef.current) event.preventDefault()
        }}
        onPointerDownOutside={(event) => {
          if (pendingRef.current) event.preventDefault()
        }}
        className="command-palette top-[max(16px,calc((100dvh-min(504px,90dvh))/2))]! w-[min(520px,92vw)]! max-w-none! translate-y-0! gap-1 overflow-hidden rounded-lg border-0 bg-popover p-1 text-sm shadow-float ring-0"
      >
        <DialogTitle className="sr-only">{t('navigation:appOverlays.commandPalette')}</DialogTitle>
        <DialogDescription className="sr-only">
          {t('navigation:appOverlays.searchPagesChatsOrActions')}
        </DialogDescription>
        <label className="flex min-h-11 shrink-0 items-center gap-2 border-b border-stroke-soft px-3 text-content-muted">
          <Search className="size-4 shrink-0" aria-hidden="true" />
          <input
            ref={inputRef}
            autoFocus
            role="combobox"
            aria-label={t('navigation:appOverlays.searchPagesChatsOrActions')}
            aria-autocomplete="list"
            aria-expanded="true"
            aria-controls={listId}
            aria-activedescendant={selectedEntry ? `${listId}-${selectedIndex}` : undefined}
            className="h-11 min-w-0 flex-1 border-0 bg-transparent text-base text-foreground caret-focus outline-none"
            value={query}
            readOnly={Boolean(pendingEntryId)}
            onChange={(event) => {
              setQuery(event.target.value)
              setActiveId(null)
              setPointerSnapshot(null)
              setOpenError('')
            }}
            onCompositionStart={() => {
              composing.current = true
            }}
            onCompositionEnd={() => {
              composing.current = false
            }}
            onKeyDown={onKeyDown}
            placeholder={t('navigation:appOverlays.searchPagesChatsOrActions')}
          />
          {pendingEntryId ? (
            <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />
          ) : (
            <Kbd>Esc</Kbd>
          )}
        </label>
        {pendingEntryId && (
          <span role="status" className="sr-only">
            {pendingEntryId.startsWith('label:')
              ? t('navigation:appOverlays.locatingLabel')
              : t('common:pageTemplate.loading')}
          </span>
        )}
        {openError && (
          <div role="alert" className="px-3 py-2 text-sm text-destructive">
            {openError}
          </div>
        )}
        <div
          id={listId}
          ref={listRef}
          className="max-h-[min(440px,max(120px,calc(90dvh-64px)))] overflow-y-auto overscroll-contain p-1"
          role="listbox"
          aria-busy={searching}
          aria-label={t('navigation:appOverlays.commandPalette')}
          onPointerMove={(event) => {
            const previous = pointerPosition.current
            pointerPosition.current = { x: event.clientX, y: event.clientY }
            if (
              event.pointerType !== 'touch' &&
              entries.length &&
              (event.clientX !== previous.x || event.clientY !== previous.y) &&
              pointerSnapshot?.query !== query
            )
              setPointerSnapshot({ query, entries })
          }}
          onPointerLeave={() => setPointerSnapshot(null)}
          onPointerCancel={() => setPointerSnapshot(null)}
        >
          {(['actions', 'pages', 'labels', 'sessions'] as const).map((group) => {
            const items = entries.filter((entry) => entry.group === group)
            if (!items.length) return null
            return (
              <div role="group" aria-label={groups[group]} key={group}>
                <div className="flex items-center gap-2 px-2 py-1.5 text-sm text-muted-foreground">
                  {groups[group]}
                  {group === 'labels' && labelsSearching && (
                    <LoaderCircle className="size-3 animate-spin" aria-hidden="true" />
                  )}
                </div>
                {items.map((entry) => {
                  const index = entries.indexOf(entry)
                  return (
                    <button
                      id={`${listId}-${index}`}
                      type="button"
                      tabIndex={-1}
                      role="option"
                      aria-selected={index === selectedIndex}
                      aria-disabled={Boolean(pendingEntryId)}
                      aria-busy={pendingEntryId === entry.id}
                      key={entry.id}
                      title={entry.title}
                      className={cn(
                        // 当前项用实心表面而非半透明，配合 30px 行高与右侧等宽提示，保持仪器式密度；
                        // 手机与粗指针下行高抬到 44px 触控下限。
                        'palette-item flex min-h-[30px] w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-[13px] text-content-secondary outline-none hover:bg-surface-hover hover:text-content focus-visible:bg-surface-hover aria-disabled:opacity-40 max-[650px]:min-h-11 pointer-coarse:min-h-11 [&_svg]:text-content-muted',
                        index === selectedIndex && 'bg-surface-hover text-content',
                      )}
                      onPointerMove={() => {
                        if (!pendingRef.current) setActiveId(entry.id)
                      }}
                      onMouseDown={(event) => event.preventDefault()}
                      onClick={() => void runEntry(entry)}
                    >
                      {pendingEntryId === entry.id ? (
                        <LoaderCircle className="size-4 shrink-0 animate-spin" aria-hidden="true" />
                      ) : (
                        <entry.Icon className="size-4 shrink-0" aria-hidden="true" />
                      )}
                      <span className="min-w-0 flex-1">
                        <span className="block truncate">{highlight(entry.label)}</span>
                        {entry.meta && (
                          <span className="block truncate text-xs text-muted-foreground">
                            {entry.meta}
                          </span>
                        )}
                      </span>
                      <span className="w-24 shrink-0 truncate text-right text-xs text-content-muted tabular-nums">
                        {entry.hint}
                      </span>
                    </button>
                  )
                })}
              </div>
            )
          })}
          {searching && (
            <div role="status" className="px-3 py-2 text-sm text-muted-foreground">
              {t('navigation:appOverlays.searchingLabels')}
            </div>
          )}
          {!entries.length && !searching && (
            <div role="status" className="px-3 py-6 text-center text-sm text-muted-foreground">
              {t('navigation:appOverlays.noMatchingResults')}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}

type QuickCreateProps = {
  type: string
  close: () => void
  notify: (message: string) => void
}

export function QuickCreate({ type, close, notify }: QuickCreateProps) {
  const { t } = useI18n()
  const titles: Record<string, string> = {
    chat: t('navigation:appOverlays.newChat2'),
    assets: t('navigation:appOverlays.exportAsset'),
    channels: t('navigation:appOverlays.connectChannel'),
    schedules: t('navigation:appOverlays.newScheduledTask'),
    config: t('navigation:appOverlays.quickSetup'),
    plugins: t('navigation:appOverlays.savePluginPolicy'),
    memory: t('navigation:appOverlays.addMemory'),
    mcp: t('navigation:appOverlays.addMCPService'),
    skills: t('navigation:appOverlays.installSkill'),
  }
  const title = titles[type] || t('navigation:appOverlays.newProject')
  const options = [
    t('navigation:appOverlays.default'),
    t('navigation:appOverlays.custom'),
    t('navigation:appOverlays.createFromTemplate'),
  ]
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    notify(t('navigation:appOverlays.actionSucceeded', { action: title }))
    close()
  }
  return (
    <Dialog open onOpenChange={(open) => !open && close()}>
      <DialogContent
        showCloseButton={false}
        className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] max-[650px]:max-h-[calc(100dvh_-_16px)] max-w-[430px]! gap-0 rounded-dialog border-0 bg-popover p-5 shadow-float ring-0"
      >
        <form onSubmit={submit}>
          <AppCardHeader>
            <div>
              <DialogTitle>{title}</DialogTitle>
              <DialogDescription>
                {t('navigation:appOverlays.enterTheBasicDetailsToContinue')}
              </DialogDescription>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={t('navigation:appOverlays.closeDialog')}
              title={t('navigation:appOverlays.closeDialog')}
              onClick={close}
            >
              <X className="size-[17px]" />
            </Button>
          </AppCardHeader>
          <FieldLabel variant="control" className="items-start">
            {t('navigation:appOverlays.name')}
            <Input name="name" placeholder={t('navigation:appOverlays.enterAName')} />
          </FieldLabel>
          <FieldLabel variant="control" className="items-start">
            {t('navigation:appOverlays.description')}
            <Input
              name="description"
              placeholder={t('navigation:appOverlays.addAShortDescription')}
            />
          </FieldLabel>
          <FieldLabel variant="control" className="items-start" htmlFor="quick-create-type">
            {t('navigation:appOverlays.type')}
          </FieldLabel>
          <select
            id="quick-create-type"
            name="type"
            defaultValue={options[0]}
            className="mt-1 h-[34px] w-full rounded-sm border border-control bg-panel px-2.5 text-[13px]"
          >
            {options.map((option) => (
              <option value={option} key={option}>
                {option}
              </option>
            ))}
          </select>
          <div className="mt-5 flex justify-end gap-2">
            <Button type="button" variant="secondary" onClick={close}>
              {t('navigation:appOverlays.cancel')}
            </Button>
            <Button type="submit">
              <Plus className="size-3.5" />
              {t('navigation:appOverlays.create')}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  )
}
