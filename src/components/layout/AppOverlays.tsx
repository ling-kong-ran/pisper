// 全站浮层：命令面板（⌘K，导航 / 搜索 / 直达），懒加载并按需挂载，避免常驻开销；搜索会话走 React Query 缓存。
// 方向 A 的样式（规格 2.5、6.6，效果图 extra-palette-dark.png）：620 宽、r-lg 弹层，52 高的搜索行，
// 分组依次是 代理与会话 · 会话标记 · 操作 · 页面（组间细线），36 高的结果行（图标 · 名称 · 等宽附注 · 右侧时间或快捷键），
// 底部一行键位提示。原先只在页头上的“快速创建”假表单（提交即提示成功）随共享页头的主操作一起删除。
import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from 'react'
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
  type LucideIcon,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { startupQueryOptions } from '@/lib/startup-queries'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Kbd } from '@/components/ui/kbd'
import { relativeTime } from '@/lib/format'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { chatApi, type SessionTreeLabelMatch } from '@/features/chat/chat-api'
import { takeCommandPaletteQuery } from '@/features/chat/events'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'

type Navigation = Array<[string, Array<[string, string, LucideIcon]>]>

type SessionSummary = {
  id: string
  name?: string
  firstMessage?: string
  cwd?: string
  modified: string
}

// 结果分组的显示顺序（效果图）：先是代理与会话、会话标记，再是操作与页面。
const GROUP_ORDER = ['sessions', 'labels', 'actions', 'pages'] as const

// 工作目录的最后一段作为等宽附注（项目名），完整路径留在 title 里。
function projectName(cwd?: string) {
  const trimmed = (cwd || '').replace(/[\\/]+$/, '')
  return trimmed.slice(Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\')) + 1)
}

type CommandEntry = {
  id: string
  Icon: LucideIcon
  label: string
  hint: string
  meta?: string
  // 名称后面的等宽附注（项目名）；meta 是名称下面的第二行。
  tag?: string
  // 右侧的快捷键提示（键帽样式），与 hint 二选一显示。
  shortcut?: string
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
  const newTaskShortcut = useShortcutLabel('primaryAction')
  const terminalShortcut = useShortcutLabel('toggleTerminal')
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
    sessions: t('navigation:appOverlays.agentsAndSessions'),
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
    // “新任务”打开派发对话框（规格 2.5）；旧名“新会话”仍能搜到。
    const newTaskLabel = t('navigation:appOverlays.newTask')
    if (
      matches(
        newTaskLabel,
        t('navigation:appOverlays.newChat'),
        t('navigation:appOverlays.actions'),
      )
    ) {
      result.push({
        id: 'action:new-chat',
        group: 'actions',
        Icon: Plus,
        label: newTaskLabel,
        hint: t('navigation:appOverlays.actions'),
        shortcut: newTaskShortcut,
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
        shortcut: terminalShortcut,
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
        tag: projectName(session.cwd) || undefined,
        title: session.cwd || undefined,
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
    // 方向键按显示顺序走：结果先按分组顺序排好，渲染与 aria-activedescendant 用同一个下标。
    return GROUP_ORDER.flatMap((group) => result.filter((entry) => entry.group === group))
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
    terminalShortcut,
    newTaskShortcut,
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
        data-command-palette
        onEscapeKeyDown={(event) => {
          if (composing.current || pendingRef.current) event.preventDefault()
        }}
        onPointerDownOutside={(event) => {
          if (pendingRef.current) event.preventDefault()
        }}
        // 620 宽、r-lg、只有弹层有阴影（规格 6.1、6.4）；顶部在视口 1/9 处（效果图），矮窗口里不低于 16px。
        className="command-palette top-[max(16px,min(11vh,calc((100dvh-min(560px,90dvh))/2)))]! flex w-[min(620px,calc(100vw-32px))]! max-w-none! translate-y-0! flex-col gap-0 overflow-hidden rounded-[var(--r-lg)] border border-stroke bg-solid p-0 text-sm shadow-float ring-0 max-[650px]:w-[calc(100vw-16px)]!"
      >
        <DialogTitle className="sr-only">{t('navigation:appOverlays.commandPalette')}</DialogTitle>
        <DialogDescription className="sr-only">
          {t('navigation:appOverlays.searchPagesChatsOrActions')}
        </DialogDescription>
        <label className="flex min-h-[52px] shrink-0 items-center gap-2.5 border-b border-stroke-soft px-4 text-content-muted">
          <Search className="size-[18px] shrink-0" strokeWidth={1.75} aria-hidden="true" />
          <input
            ref={inputRef}
            autoFocus
            role="combobox"
            aria-label={t('navigation:appOverlays.searchPagesChatsOrActions')}
            aria-autocomplete="list"
            aria-expanded="true"
            aria-controls={listId}
            aria-activedescendant={selectedEntry ? `${listId}-${selectedIndex}` : undefined}
            className="h-[52px] min-w-0 flex-1 border-0 bg-transparent text-base text-content caret-focus outline-none placeholder:text-content-muted"
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
          {pendingEntryId && <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />}
        </label>
        {pendingEntryId && (
          <span role="status" className="sr-only">
            {pendingEntryId.startsWith('label:')
              ? t('navigation:appOverlays.locatingLabel')
              : t('common:pageTemplate.loading')}
          </span>
        )}
        {openError && (
          <div role="alert" className="border-b border-stroke-soft px-4 py-2 text-sm text-content">
            {openError}
          </div>
        )}
        <div
          id={listId}
          ref={listRef}
          className="max-h-[min(440px,max(120px,calc(90dvh-104px)))] overflow-y-auto overscroll-contain px-2 pb-2"
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
          // 离开列表时忘掉上次的指针位置：再次进入（哪怕落在同一点）也算一次进入并冻结结果，
          // 否则会话组在最前、行位置不变时，回到原处不会触发冻结，迟到的标记结果会插进来。
          onPointerLeave={() => {
            pointerPosition.current = { x: -1, y: -1 }
            setPointerSnapshot(null)
          }}
          onPointerCancel={() => setPointerSnapshot(null)}
        >
          {GROUP_ORDER.map((group) => {
            const items = entries.filter((entry) => entry.group === group)
            if (!items.length) return null
            return (
              <div
                role="group"
                aria-label={groups[group]}
                key={group}
                // 组与组之间一条细线（-mx-2 铺满弹层宽度），第一组没有。
                className="-mx-2 px-2 [&+&]:border-t [&+&]:border-stroke-soft"
              >
                <div className="flex items-center gap-2 px-2.5 pt-2.5 pb-1 text-xs text-content-muted">
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
                        // 36 高的结果行，当前项是实心悬停底（不用强调色）；手机与粗指针下抬到 44px 触控下限。
                        'palette-item flex min-h-9 w-full items-center gap-2.5 rounded-[var(--r-md)] px-2.5 py-1.5 text-left text-sm text-content outline-none hover:bg-surface-hover focus-visible:bg-surface-hover aria-disabled:opacity-40 max-[650px]:min-h-11 pointer-coarse:min-h-11 [&_svg]:text-content-muted',
                        index === selectedIndex && 'bg-surface-hover',
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
                        <entry.Icon
                          className="size-4 shrink-0"
                          strokeWidth={1.75}
                          aria-hidden="true"
                        />
                      )}
                      <span className="min-w-0 flex-1">
                        <span className="flex min-w-0 items-baseline gap-2">
                          <span className="min-w-0 truncate">{highlight(entry.label)}</span>
                          {entry.tag && (
                            <span className="min-w-0 shrink-[2] truncate font-mono text-xs text-content-muted">
                              {entry.tag}
                            </span>
                          )}
                        </span>
                        {entry.meta && (
                          <span className="block truncate text-xs text-content-muted">
                            {entry.meta}
                          </span>
                        )}
                      </span>
                      {entry.shortcut ? (
                        <Kbd
                          aria-hidden="true"
                          className="shrink-0 border-transparent bg-transparent text-content-muted shadow-none pointer-coarse:hidden"
                        >
                          {entry.shortcut}
                        </Kbd>
                      ) : (
                        <span className="max-w-28 shrink-0 truncate text-right text-xs text-content-muted tabular-nums">
                          {entry.hint}
                        </span>
                      )}
                    </button>
                  )
                })}
              </div>
            )
          })}
          {searching && (
            <div role="status" className="px-2.5 py-2 text-sm text-content-muted">
              {t('navigation:appOverlays.searchingLabels')}
            </div>
          )}
          {!entries.length && !searching && (
            <div role="status" className="px-3 py-6 text-center text-sm text-content-muted">
              {t('navigation:appOverlays.noMatchingResults')}
            </div>
          )}
        </div>
        {/* 底部键位提示（效果图）：读屏已经从 combobox / listbox 的语义里知道怎么操作，这一行只给眼睛看。 */}
        <div
          aria-hidden="true"
          data-palette-hints
          className="flex h-9 flex-none items-center gap-3 border-t border-stroke-soft px-4 text-xs text-content-muted pointer-coarse:hidden max-[650px]:hidden"
        >
          <span>↑↓ {t('navigation:appOverlays.hintSelect')}</span>
          <span>↵ {t('navigation:appOverlays.hintOpen')}</span>
          <span>Tab {t('navigation:appOverlays.hintGroups')}</span>
          <span className="ml-auto">Esc {t('navigation:appOverlays.hintClose')}</span>
        </div>
      </DialogContent>
    </Dialog>
  )
}
