// 群聊页的两个对话框：群设置（名字、目标、成员、项目目录、维护简报的 Bot）与手改简报
// （已定一行一条；待办一行一条，开头写 [x] 表示做完，写 @名字 指定负责的人）。
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { FolderOpen } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { pickSystemDirectory } from '@/lib/pick-system-directory'
import { BotAvatar } from './BotAvatar'
import type { Bot } from './bots-api'
import { parseTodoLine, todoLine } from './room-presentation'
import type { Room, RoomBrief, RoomMember, RoomTodo } from './rooms-api'

const MAX_MEMBERS = 8

export type RoomSettingsDraft = Pick<Room, 'name' | 'goal' | 'members' | 'cwd' | 'briefBotId'>

export function RoomSettingsDialog({
  open,
  onOpenChange,
  room,
  bots,
  onSave,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  room: Room
  bots: readonly Bot[]
  onSave: (draft: RoomSettingsDraft) => Promise<void>
}) {
  const { t } = useI18n()
  const [draft, setDraft] = useState<RoomSettingsDraft>(room)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [picking, setPicking] = useState(false)
  const [browsing, setBrowsing] = useState(false)
  const wasOpen = useRef(false)

  // 只在打开的那一下填表：群详情每一两秒轮询一次，跟着它重填会把正在打的字清掉。
  useEffect(() => {
    if (open && !wasOpen.current) {
      setDraft({
        name: room.name,
        goal: room.goal,
        members: room.members,
        cwd: room.cwd,
        briefBotId: room.briefBotId,
      })
      setError('')
    }
    wasOpen.current = open
  }, [open, room])

  // 先开系统的文件夹对话框；开不了（手机、配对设备）时退回目录面板逐级点选，不用手打路径。
  const pickFolder = async () => {
    if (picking) return
    setPicking(true)
    setError('')
    try {
      const result = await pickSystemDirectory(draft.cwd.trim(), t('bots:room.settings.folder'))
      if (result.status === 'picked') setDraft((current) => ({ ...current, cwd: result.path }))
      else if (result.status === 'unavailable') setBrowsing(true)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setPicking(false)
    }
  }

  const toggle = (id: string, checked: boolean) =>
    setDraft((current) => {
      const members = checked
        ? [...current.members, id].slice(0, MAX_MEMBERS)
        : current.members.filter((item) => item !== id)
      return {
        ...current,
        members,
        briefBotId: members.includes(current.briefBotId) ? current.briefBotId : '',
      }
    })

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (!draft.name.trim() || !draft.members.length) return
    setSaving(true)
    setError('')
    try {
      await onSave({
        ...draft,
        name: draft.name.trim(),
        goal: draft.goal.trim(),
        cwd: draft.cwd.trim(),
      })
      onOpenChange(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <form onSubmit={submit} className="contents">
          <DialogHeader>
            <DialogTitle>{t('bots:room.settings.title')}</DialogTitle>
            <DialogDescription>{t('bots:room.settings.description')}</DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <FieldLabel htmlFor="room-settings-name">{t('bots:room.new.name')}</FieldLabel>
              <Input
                id="room-settings-name"
                value={draft.name}
                maxLength={40}
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <FieldLabel htmlFor="room-settings-goal">{t('bots:room.new.goal')}</FieldLabel>
              <Input
                id="room-settings-goal"
                value={draft.goal}
                maxLength={200}
                placeholder={t('bots:room.new.goalPlaceholder')}
                onChange={(event) => setDraft({ ...draft, goal: event.target.value })}
              />
            </div>
            <fieldset className="flex min-w-0 flex-col">
              <legend className="mb-1.5 text-sm font-medium text-content">
                {t('bots:room.new.members', { count: draft.members.length, max: MAX_MEMBERS })}
              </legend>
              <ul className="m-0 flex max-h-48 list-none flex-col gap-0.5 overflow-y-auto p-0">
                {bots.map((bot) => {
                  const id = `room-settings-member-${bot.id}`
                  const checked = draft.members.includes(bot.id)
                  return (
                    <li key={bot.id}>
                      <label
                        htmlFor={id}
                        className="flex h-9 items-center gap-2.5 rounded-[var(--r-md)] px-2 hover:bg-surface-hover pointer-coarse:h-11"
                      >
                        <Checkbox
                          id={id}
                          checked={checked}
                          disabled={!checked && draft.members.length >= MAX_MEMBERS}
                          onCheckedChange={(value) => toggle(bot.id, value === true)}
                        />
                        <BotAvatar avatar={bot.avatar} size={20} animate={false} />
                        <span className="min-w-0 flex-1 truncate text-sm">{bot.name}</span>
                      </label>
                    </li>
                  )
                })}
              </ul>
            </fieldset>
            <div className="flex flex-col gap-1.5">
              <FieldLabel htmlFor="room-settings-folder">
                {t('bots:room.settings.folder')}
              </FieldLabel>
              <span className="flex gap-2">
                <Input
                  id="room-settings-folder"
                  value={draft.cwd}
                  className="font-mono"
                  placeholder={t('bots:room.settings.folderPlaceholder')}
                  onChange={(event) => setDraft({ ...draft, cwd: event.target.value })}
                />
                <Button
                  type="button"
                  variant="outline"
                  disabled={picking}
                  onClick={() => void pickFolder()}
                >
                  <FolderOpen aria-hidden="true" />
                  {t('bots:new.pick')}
                </Button>
              </span>
              <p className="m-0 text-xs text-content-muted">{t('bots:room.settings.folderHint')}</p>
            </div>
            <div className="flex flex-col gap-1.5">
              <FieldLabel id="room-settings-brief-label">
                {t('bots:room.settings.briefBot')}
              </FieldLabel>
              <AppSelect
                aria-labelledby="room-settings-brief-label"
                value={draft.briefBotId}
                onChange={(event) => setDraft({ ...draft, briefBotId: event.target.value })}
              >
                <option value="">{t('bots:room.settings.briefBotNone')}</option>
                {bots
                  .filter((bot) => draft.members.includes(bot.id))
                  .map((bot) => (
                    <option key={bot.id} value={bot.id}>
                      {bot.name}
                    </option>
                  ))}
              </AppSelect>
              <p className="m-0 text-xs text-content-muted">
                {t('bots:room.settings.briefBotHint')}
              </p>
            </div>
            {error && (
              <p role="alert" className="m-0 text-sm text-destructive">
                {error}
              </p>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              {t('common:ui.cancel')}
            </Button>
            <Button type="submit" disabled={saving || !draft.name.trim() || !draft.members.length}>
              {t('bots:room.settings.save')}
            </Button>
          </DialogFooter>
        </form>
        <WorkspacePicker
          open={browsing}
          initialPath={draft.cwd}
          onOpenChange={setBrowsing}
          onSelect={(path) => {
            setDraft((current) => ({ ...current, cwd: path }))
            setBrowsing(false)
          }}
        />
      </DialogContent>
    </Dialog>
  )
}

export function RoomBriefDialog({
  open,
  onOpenChange,
  brief,
  members,
  onSave,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  brief: RoomBrief | null
  members: readonly RoomMember[]
  onSave: (brief: Pick<RoomBrief, 'decisions' | 'todos'>) => Promise<void>
}) {
  const { t } = useI18n()
  const [decisions, setDecisions] = useState('')
  const [todos, setTodos] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const wasOpen = useRef(false)

  // 同上：只在打开的那一下填，轮询带来的新简报不覆盖正在改的字。
  useEffect(() => {
    if (open && !wasOpen.current) {
      setDecisions((brief?.decisions || []).join('\n'))
      setTodos((brief?.todos || []).map((todo) => todoLine(todo, members)).join('\n'))
      setError('')
    }
    wasOpen.current = open
  }, [open, brief, members])

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setSaving(true)
    setError('')
    try {
      await onSave({
        decisions: decisions
          .split('\n')
          .map((line) => line.trim())
          .filter(Boolean),
        todos: todos
          .split('\n')
          .map((line) => parseTodoLine(line, members))
          .filter((todo): todo is RoomTodo => Boolean(todo)),
      })
      onOpenChange(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <form onSubmit={submit} className="contents">
          <DialogHeader>
            <DialogTitle>{t('bots:room.brief.editTitle')}</DialogTitle>
            <DialogDescription>{t('bots:room.brief.editDescription')}</DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <FieldLabel htmlFor="room-brief-decisions">
                {t('bots:room.brief.decisionsLabel')}
              </FieldLabel>
              <Textarea
                id="room-brief-decisions"
                rows={4}
                value={decisions}
                onChange={(event) => setDecisions(event.target.value)}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <FieldLabel htmlFor="room-brief-todos">{t('bots:room.brief.todosLabel')}</FieldLabel>
              <Textarea
                id="room-brief-todos"
                rows={5}
                value={todos}
                onChange={(event) => setTodos(event.target.value)}
              />
            </div>
            {error && (
              <p role="alert" className="m-0 text-sm text-destructive">
                {error}
              </p>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              {t('common:ui.cancel')}
            </Button>
            <Button type="submit" disabled={saving}>
              {t('bots:room.brief.save')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
