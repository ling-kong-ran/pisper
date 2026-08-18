// 全站浮层：命令面板（Cmd+K，导航/搜索/直达）与快捷创建（新会话/资源）。
// 两个都懒加载并按需挂载，避免常驻开销；搜索会话走 React Query 缓存。
import { useEffect, useMemo, useState, type FormEvent, type KeyboardEvent } from 'react'
import { LoaderCircle, MessageSquare, Plus, Search, Tag, X, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { FieldLabel } from '@/components/ui/field'
import { relativeTime } from '@/lib/format'
import { chatApi, type SessionTreeLabelMatch } from '@/features/chat/chat-api'

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
  run: () => void | Promise<void>
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
}

export function CommandPalette({
  navigation,
  onClose,
  onNavigate,
  onOpenSession,
  onNewChat,
}: CommandPaletteProps) {
  const { t, language } = useI18n()
  const [query, setQuery] = useState('')
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [labelMatches, setLabelMatches] = useState<SessionTreeLabelMatch[]>([])
  const [labelsSearching, setLabelsSearching] = useState(false)
  const [activeIndex, setActiveIndex] = useState(0)
  const [pendingEntryId, setPendingEntryId] = useState('')

  useEffect(() => {
    let active = true
    apiJson<{ sessions?: SessionSummary[] }>('/api/sessions')
      .then((data) => {
        if (active) setSessions(data.sessions || [])
      })
      .catch(() => {})
    return () => {
      active = false
    }
  }, [])

  useEffect(() => {
    const keyword = query.trim()
    if (!keyword) {
      setLabelMatches([])
      setLabelsSearching(false)
      return undefined
    }
    let active = true
    setLabelsSearching(true)
    const timer = window.setTimeout(() => {
      void chatApi
        .searchSessionTreeLabels(keyword, 12)
        .then((data) => {
          if (!active) return
          setLabelMatches(data.labels || [])
          setLabelsSearching(false)
        })
        .catch(() => {
          if (!active) return
          setLabelMatches([])
          setLabelsSearching(false)
        })
    }, 140)
    return () => {
      active = false
      window.clearTimeout(timer)
    }
  }, [query])

  const entries = useMemo(() => {
    const keyword = query.trim().toLocaleLowerCase(language)
    const matches = (...values: unknown[]) =>
      !keyword ||
      values.some((value) =>
        String(value || '')
          .toLocaleLowerCase(language)
          .includes(keyword),
      )
    const result: CommandEntry[] = []
    const newChatLabel = t('navigation:appOverlays.newChat')
    if (matches(newChatLabel, t('navigation:appOverlays.actions'))) {
      result.push({
        id: 'action:new-chat',
        Icon: Plus,
        label: newChatLabel,
        hint: t('navigation:appOverlays.actions'),
        run: onNewChat,
      })
    }
    for (const [group, items] of navigation) {
      for (const [id, label, Icon] of items) {
        if (matches(label, group, id)) {
          result.push({ id: `page:${id}`, Icon, label, hint: group, run: () => onNavigate(id) })
        }
      }
    }
    const formatTimestamp = (value: string) => {
      const timestamp = Date.parse(value)
      if (!Number.isFinite(timestamp)) return t('navigation:appOverlays.unknownTime')
      return new Intl.DateTimeFormat(language, {
        dateStyle: 'medium',
        timeStyle: 'short',
      }).format(timestamp)
    }
    for (const label of labelMatches) {
      const sessionTime = formatTimestamp(label.sessionCreated || label.sessionModified)
      const nodeTime = formatTimestamp(label.nodeTimestamp)
      result.push({
        id: `label:${label.sessionId}:${label.entryId}`,
        Icon: Tag,
        label: label.label,
        hint: label.sessionName || t('navigation:appOverlays.untitledChat'),
        meta: t('navigation:appOverlays.labelResultTimes', { sessionTime, nodeTime }),
        title: label.summary || label.label,
        run: () => onOpenSession(label.sessionId, label.entryId, label.active),
      })
    }
    for (const session of [...sessions].sort(
      (a, b) => Date.parse(b.modified) - Date.parse(a.modified),
    )) {
      if (!matches(session.name, session.firstMessage)) continue
      result.push({
        id: `session:${session.id}`,
        Icon: MessageSquare,
        label: session.name || t('navigation:appOverlays.untitledChat'),
        hint: relativeTime(session.modified, language),
        run: () => onOpenSession(session.id),
      })
      if (result.filter((entry) => entry.id.startsWith('session:')).length >= 8) break
    }
    return result
  }, [labelMatches, language, navigation, onNavigate, onNewChat, onOpenSession, query, sessions, t])

  useEffect(() => {
    setActiveIndex(0)
  }, [query])
  const selectedIndex = Math.min(activeIndex, Math.max(0, entries.length - 1))
  const runEntry = async (entry?: CommandEntry) => {
    if (!entry || pendingEntryId) return
    if (!entry.id.startsWith('label:')) {
      onClose()
      void entry.run()
      return
    }
    setPendingEntryId(entry.id)
    try {
      await entry.run()
      onClose()
    } catch {
      setPendingEntryId('')
    }
  }
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (pendingEntryId) {
      event.preventDefault()
      return
    }
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      setActiveIndex((current) => (entries.length ? (current + 1) % entries.length : 0))
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      setActiveIndex((current) =>
        entries.length ? (current - 1 + entries.length) % entries.length : 0,
      )
    } else if (event.key === 'Enter') {
      event.preventDefault()
      void runEntry(entries[selectedIndex])
    } else if (event.key === 'Escape') {
      event.preventDefault()
      onClose()
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && !pendingEntryId && onClose()}>
      <DialogContent
        showCloseButton={false}
        aria-busy={Boolean(pendingEntryId)}
        className="command-palette max-[650px]:w-full w-[min(560px,calc(100vw_-_40px))] overflow-hidden [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] shadow-[0_28px_80px_-24px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] top-[16vh]! max-w-[calc(100vw-40px)]! translate-y-0! gap-0 p-0 ring-0 max-sm:top-[8vh]! sm:max-w-[560px]!"
      >
        <DialogTitle className="sr-only">{t('navigation:appOverlays.commandPalette')}</DialogTitle>
        <DialogDescription className="sr-only">
          {t('navigation:appOverlays.searchPagesChatsOrActions')}
        </DialogDescription>
        <label className="palette-input [&_input]:w-full [&_input]:min-w-0 [&_input]:border-0 [&_input]:[outline:0] [&_input]:bg-transparent [&_input]:text-[var(--text)] [&_input]:text-[14px] [&_kbd]:flex-none [&_kbd]:[border:1px_solid_var(--stroke)] [&_kbd]:rounded-[5px] [&_kbd]:bg-[var(--surface-subtle)] [&_kbd]:p-[1px_5px] [&_kbd]:text-[var(--text-muted)] [&_kbd]:font-[inherit] [&_kbd]:text-[10px] flex h-[46px] items-center gap-[9px] [border-bottom:1px_solid_var(--stroke-soft)] [padding:0_14px] text-[var(--text-muted)]">
          <Search size={16} />
          <input
            autoFocus
            value={query}
            disabled={Boolean(pendingEntryId)}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={onKeyDown}
            placeholder={t('navigation:appOverlays.searchPagesChatsOrActions')}
          />
          {pendingEntryId ? (
            <span
              className="flex shrink-0 items-center gap-1.5 text-xs"
              role="status"
              aria-live="polite"
            >
              <LoaderCircle className="size-3.5 animate-spin" aria-hidden="true" />
              {t('navigation:appOverlays.locatingLabel')}
            </span>
          ) : (
            <kbd>Esc</kbd>
          )}
        </label>
        <div
          className="max-h-[min(380px,50vh)] overflow-y-auto [padding:6px]"
          role="listbox"
          aria-label={t('navigation:appOverlays.commandPalette')}
        >
          {entries.map((entry, index) => (
            <button
              type="button"
              className={`palette-item [&_>_svg]:flex-none [&_>_svg]:text-[var(--text-muted)] [&.active]:bg-[var(--star-soft)] [&.active]:text-[var(--text)] [&.active_>_svg]:text-[var(--star-strong)] flex w-full items-center gap-[10px] border-0 rounded-[var(--r-sm)] bg-transparent [padding:9px_10px] text-[var(--text-secondary)] text-[13px] text-left cursor-pointer ${entry.meta ? 'has-meta' : ''}    ${index === selectedIndex ? 'active' : ''}`}
              title={entry.title}
              role="option"
              aria-selected={index === selectedIndex}
              disabled={Boolean(pendingEntryId)}
              aria-busy={pendingEntryId === entry.id}
              onMouseEnter={() => !pendingEntryId && setActiveIndex(index)}
              onClick={() => void runEntry(entry)}
              key={entry.id}
            >
              {pendingEntryId === entry.id ? (
                <LoaderCircle className="size-[15px] animate-spin" aria-hidden="true" />
              ) : (
                <entry.Icon size={15} />
              )}
              <span className="palette-item-copy [&_>_small]:overflow-hidden [&_>_small]:text-[var(--text-muted)] [&_>_small]:text-[10px] [&_>_small]:text-ellipsis [&_>_small]:whitespace-nowrap flex min-w-0 flex-1 flex-col gap-[2px]">
                <span className="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">
                  {entry.label}
                </span>
                {entry.meta && <small>{entry.meta}</small>}
              </span>
              <span className="palette-item-hint max-[650px]:max-w-[34vw] max-w-[180px] flex-none overflow-hidden text-[var(--text-muted)] text-[11px] text-ellipsis whitespace-nowrap">
                {pendingEntryId === entry.id
                  ? t('navigation:appOverlays.locatingLabel')
                  : entry.hint}
              </span>
            </button>
          ))}
          {!entries.length && (
            <div className="palette-empty [padding:20px] text-[var(--text-muted)] text-[12px] text-center">
              {labelsSearching
                ? t('navigation:appOverlays.searchingLabels')
                : t('navigation:appOverlays.noMatchingResults')}
            </div>
          )}
          {entries.length > 0 && labelsSearching && (
            <div
              aria-live="polite"
              className="palette-empty [padding:20px] text-[var(--text-muted)] text-[12px] text-center palette-searching [.palette-empty&]:p-[8px_12px]"
            >
              {t('navigation:appOverlays.searchingLabels')}
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
    config: t('navigation:appOverlays.addProvider'),
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
        className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)] max-w-[430px]! gap-0 rounded-dialog border-surface-highlight shadow-dialog ring-0"
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
            className="mt-1 w-full bg-surface-subtle"
          >
            {options.map((option) => (
              <option value={option} key={option}>
                {option}
              </option>
            ))}
          </select>
          <div className="flex justify-end gap-[8px] [margin-top:18px]">
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
