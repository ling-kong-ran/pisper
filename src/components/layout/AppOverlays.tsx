import { useEffect, useMemo, useState, type FormEvent, type KeyboardEvent } from 'react'
import { MessageSquare, Plus, Search, Tag, X, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { relativeTime } from '@/lib/format'
import { chatApi, type SessionTreeLabelMatch } from '@/features/chat/chat-api'

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
  run: () => void
}

type CommandPaletteProps = {
  navigation: Navigation
  onClose: () => void
  onNavigate: (page: string) => void
  onOpenSession: (sessionId: string, targetEntryId?: string, targetActive?: boolean) => void
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
  const runEntry = (entry?: CommandEntry) => {
    if (!entry) return
    onClose()
    entry.run()
  }
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
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
      runEntry(entries[selectedIndex])
    } else if (event.key === 'Escape') {
      event.preventDefault()
      onClose()
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        showCloseButton={false}
        className="command-palette top-[16vh]! max-w-[calc(100vw-40px)]! translate-y-0! gap-0 p-0 ring-0 max-sm:top-[8vh]! sm:max-w-[560px]!"
      >
        <DialogTitle className="sr-only">{t('navigation:appOverlays.commandPalette')}</DialogTitle>
        <DialogDescription className="sr-only">
          {t('navigation:appOverlays.searchPagesChatsOrActions')}
        </DialogDescription>
        <label className="palette-input">
          <Search size={16} />
          <input
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={onKeyDown}
            placeholder={t('navigation:appOverlays.searchPagesChatsOrActions')}
          />
          <kbd>Esc</kbd>
        </label>
        <div
          className="palette-list"
          role="listbox"
          aria-label={t('navigation:appOverlays.commandPalette')}
        >
          {entries.map((entry, index) => (
            <button
              type="button"
              className={`palette-item ${entry.meta ? 'has-meta' : ''} ${index === selectedIndex ? 'active' : ''}`}
              title={entry.title}
              role="option"
              aria-selected={index === selectedIndex}
              onMouseEnter={() => setActiveIndex(index)}
              onClick={() => runEntry(entry)}
              key={entry.id}
            >
              <entry.Icon size={15} />
              <span className="palette-item-copy">
                <span className="palette-item-label">{entry.label}</span>
                {entry.meta && <small>{entry.meta}</small>}
              </span>
              <span className="palette-item-hint">{entry.hint}</span>
            </button>
          ))}
          {!entries.length && (
            <div className="palette-empty">
              {labelsSearching
                ? t('navigation:appOverlays.searchingLabels')
                : t('navigation:appOverlays.noMatchingResults')}
            </div>
          )}
          {entries.length > 0 && labelsSearching && (
            <div aria-live="polite" className="palette-empty palette-searching">
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
        className="modal max-w-[430px]! gap-0 rounded-dialog border-surface-highlight p-[18px] shadow-dialog ring-0"
      >
        <form onSubmit={submit}>
          <div className="card-head">
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
          </div>
          <Label className="field-label items-start">
            {t('navigation:appOverlays.name')}
            <Input name="name" placeholder={t('navigation:appOverlays.enterAName')} />
          </Label>
          <Label className="field-label items-start">
            {t('navigation:appOverlays.description')}
            <Input
              name="description"
              placeholder={t('navigation:appOverlays.addAShortDescription')}
            />
          </Label>
          <Label className="field-label items-start" htmlFor="quick-create-type">
            {t('navigation:appOverlays.type')}
          </Label>
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
          <div className="modal-actions">
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
