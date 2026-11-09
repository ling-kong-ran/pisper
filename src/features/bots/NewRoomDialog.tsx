// 新建群聊：起名、勾选成员 Bot、可选的目标。项目目录、维护简报的 Bot 之后在群设置里改。
import { useEffect, useState, type FormEvent } from 'react'
import { useI18n } from '@/app/use-i18n'
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
import { InlineEmptyState } from '@/components/app/PageState'
import { BotAvatar } from './BotAvatar'
import { useBotsStore } from './bots-store'
import { createRoom, type Room } from './rooms-api'
import { useRoomsStore } from './rooms-store'

const MAX_MEMBERS = 8

export function NewRoomDialog({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  onCreated: (room: Room) => void
}) {
  const { t } = useI18n()
  const bots = useBotsStore((state) => state.bots)
  const upsert = useRoomsStore((state) => state.upsert)
  const [name, setName] = useState('')
  const [goal, setGoal] = useState('')
  const [members, setMembers] = useState<string[]>([])
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  // 每次打开都是一张新的表。
  useEffect(() => {
    if (!open) return
    setName('')
    setGoal('')
    setMembers([])
    setError('')
  }, [open])

  const toggle = (id: string, checked: boolean) =>
    setMembers((current) =>
      checked ? [...current, id].slice(0, MAX_MEMBERS) : current.filter((item) => item !== id),
    )

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (!name.trim() || !members.length) return
    setSaving(true)
    setError('')
    try {
      const room = await createRoom({ name: name.trim(), members, goal: goal.trim() })
      upsert(room)
      onOpenChange(false)
      onCreated(room)
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
            <DialogTitle>{t('bots:room.new.title')}</DialogTitle>
            <DialogDescription>{t('bots:room.new.description')}</DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <FieldLabel htmlFor="new-room-name">{t('bots:room.new.name')}</FieldLabel>
              <Input
                id="new-room-name"
                value={name}
                maxLength={40}
                autoFocus
                placeholder={t('bots:room.new.namePlaceholder')}
                onChange={(event) => setName(event.target.value)}
              />
            </div>
            <fieldset className="flex min-w-0 flex-col gap-1.5">
              <legend className="mb-1.5 text-sm font-medium text-content">
                {t('bots:room.new.members', { count: members.length, max: MAX_MEMBERS })}
              </legend>
              {bots.length ? (
                <ul className="m-0 flex max-h-56 list-none flex-col gap-0.5 overflow-y-auto p-0">
                  {bots.map((bot) => {
                    const id = `new-room-member-${bot.id}`
                    const checked = members.includes(bot.id)
                    return (
                      <li key={bot.id}>
                        <label
                          htmlFor={id}
                          className="flex h-9 items-center gap-2.5 rounded-[var(--r-md)] px-2 hover:bg-surface-hover pointer-coarse:h-11"
                        >
                          <Checkbox
                            id={id}
                            checked={checked}
                            disabled={!checked && members.length >= MAX_MEMBERS}
                            onCheckedChange={(value) => toggle(bot.id, value === true)}
                          />
                          <BotAvatar avatar={bot.avatar} size={20} animate={false} />
                          <span className="min-w-0 flex-1 truncate text-sm text-content">
                            {bot.name}
                          </span>
                          {!bot.enabled && (
                            <span className="text-xs text-content-muted">
                              {t('bots:room.paused')}
                            </span>
                          )}
                        </label>
                      </li>
                    )
                  })}
                </ul>
              ) : (
                <InlineEmptyState compact title={t('bots:room.new.noBots')} />
              )}
            </fieldset>
            <div className="flex flex-col gap-1.5">
              <FieldLabel htmlFor="new-room-goal">{t('bots:room.new.goal')}</FieldLabel>
              <Input
                id="new-room-goal"
                value={goal}
                maxLength={200}
                placeholder={t('bots:room.new.goalPlaceholder')}
                onChange={(event) => setGoal(event.target.value)}
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
            <Button type="submit" disabled={saving || !name.trim() || !members.length}>
              {t('bots:room.new.create')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
