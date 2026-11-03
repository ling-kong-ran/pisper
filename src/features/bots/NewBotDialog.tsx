// 新建 Bot（设计稿 botim-new）：只要名字、头像和它的电脑；它负责什么在对话里告诉它，或之后在详情里写。
// 第一版只有本机这一台电脑可选，所以“它的电脑”同时要选工作目录；执行节点在后续阶段加入。
import { useState, type FormEvent } from 'react'
import { FolderOpen } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { Button } from '@/components/ui/button'
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
import { cn } from '@/lib/utils'
import { BOT_CHARACTERS, BOT_COLORS, type BotAvatarValue } from './avatar'
import { BotAvatar } from './BotAvatar'
import { createBot, type Bot } from './bots-api'
import { characterLabel } from './bot-labels'
import { useBotsStore } from './bots-store'

export function NewBotDialog({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  onCreated: (bot: Bot) => void
}) {
  const { t } = useI18n()
  const upsert = useBotsStore((state) => state.upsert)
  const [name, setName] = useState('')
  const [avatar, setAvatar] = useState<BotAvatarValue>({ character: 'robot', color: 'blue' })
  const [cwd, setCwd] = useState('')
  const [picking, setPicking] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setSaving(true)
    setError('')
    try {
      const bot = await createBot({ name: name.trim(), avatar, workspace: { cwd } })
      upsert(bot)
      onOpenChange(false)
      setName('')
      onCreated(bot)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[480px]">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>{t('bots:new.title')}</DialogTitle>
            <DialogDescription>{t('bots:new.description')}</DialogDescription>
          </DialogHeader>
          <FieldLabel variant="control">
            {t('bots:new.name')}
            <Input
              value={name}
              autoFocus
              maxLength={24}
              placeholder={t('bots:new.namePlaceholder')}
              onChange={(event) => setName(event.target.value)}
            />
          </FieldLabel>
          <fieldset className="flex flex-col gap-2">
            <legend className="mb-1 text-sm font-medium text-content">{t('bots:new.look')}</legend>
            <div
              role="radiogroup"
              aria-label={t('bots:new.character')}
              className="grid grid-cols-6 gap-2"
            >
              {BOT_CHARACTERS.map((character) => (
                <button
                  key={character}
                  type="button"
                  role="radio"
                  aria-checked={avatar.character === character}
                  aria-label={characterLabel(t, character)}
                  title={characterLabel(t, character)}
                  className={cn(
                    'grid place-items-center rounded-[12px] p-0.5 ring-offset-2 ring-offset-surface-raised',
                    avatar.character === character && 'ring-2 ring-content',
                  )}
                  onClick={() => setAvatar((current) => ({ ...current, character }))}
                >
                  <BotAvatar avatar={{ ...avatar, character }} size={44} />
                </button>
              ))}
            </div>
            <div role="radiogroup" aria-label={t('bots:new.color')} className="flex gap-2">
              {BOT_COLORS.map((color) => (
                <button
                  key={color}
                  type="button"
                  role="radio"
                  aria-checked={avatar.color === color}
                  aria-label={t('bots:new.colorOption', { index: BOT_COLORS.indexOf(color) + 1 })}
                  className={cn(
                    'rounded-full p-0.5 ring-offset-2 ring-offset-surface-raised',
                    avatar.color === color && 'ring-2 ring-content',
                  )}
                  onClick={() => setAvatar((current) => ({ ...current, color }))}
                >
                  <BotAvatar avatar={{ ...avatar, color }} size={26} className="rounded-full" />
                </button>
              ))}
            </div>
          </fieldset>
          <FieldLabel variant="control">
            {t('bots:new.computer')}
            <span className="flex gap-2">
              <Input
                value={cwd}
                className="font-mono"
                placeholder={t('bots:new.cwdPlaceholder')}
                onChange={(event) => setCwd(event.target.value)}
              />
              <Button type="button" variant="outline" onClick={() => setPicking(true)}>
                <FolderOpen aria-hidden="true" />
                {t('bots:new.pick')}
              </Button>
            </span>
            <span className="text-xs text-content-muted">{t('bots:new.computerHint')}</span>
          </FieldLabel>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              {t('bots:new.cancel')}
            </Button>
            <Button type="submit" disabled={saving || !name.trim() || !cwd.trim()}>
              {saving ? t('bots:new.creating') : t('bots:new.create')}
            </Button>
          </DialogFooter>
        </form>
        <WorkspacePicker
          open={picking}
          initialPath={cwd}
          onOpenChange={setPicking}
          onSelect={(path) => {
            setCwd(path)
            setPicking(false)
          }}
        />
      </DialogContent>
    </Dialog>
  )
}
