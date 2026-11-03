// 新建 Bot（设计稿 botim-new，按“像 Grok 一样少填”简化）：只要一个名字就能开始。
// - 它负责什么：可选，写了就成为它记得的工作说明；不写就在对话里告诉它。
// - 样子：随机给一个，点头像换一个；想自己挑再展开。
// - 工作目录：不选就用本机的默认工作目录，之后在设置里能换（包括换到另一台电脑）。
import { useEffect, useState, type FormEvent } from 'react'
import { FolderOpen, Shuffle } from 'lucide-react'
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
import { Textarea } from '@/components/ui/textarea'
import { randomBotAvatar, type BotAvatarValue } from './avatar'
import { BotAvatar } from './BotAvatar'
import { BotAvatarPicker } from './BotAvatarPicker'
import { createBot, type Bot } from './bots-api'
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
  const [instructions, setInstructions] = useState('')
  const [avatar, setAvatar] = useState<BotAvatarValue>(randomBotAvatar)
  const [choosingLook, setChoosingLook] = useState(false)
  const [cwd, setCwd] = useState('')
  const [editingFolder, setEditingFolder] = useState(false)
  const [picking, setPicking] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  // 每次打开都是一张新的表：换个随机头像，收起可选项。
  useEffect(() => {
    if (!open) return
    setAvatar(randomBotAvatar())
    setChoosingLook(false)
    setEditingFolder(false)
    setError('')
  }, [open])

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setSaving(true)
    setError('')
    try {
      const bot = await createBot({
        name: name.trim(),
        avatar,
        instructions: instructions.trim(),
        workspace: { cwd: cwd.trim() },
      })
      upsert(bot)
      onOpenChange(false)
      setName('')
      setInstructions('')
      setCwd('')
      onCreated(bot)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[440px]">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>{t('bots:new.title')}</DialogTitle>
            <DialogDescription>{t('bots:new.description')}</DialogDescription>
          </DialogHeader>
          <div className="flex items-end gap-3">
            <button
              type="button"
              className="group relative flex-none rounded-[28%]"
              aria-label={t('bots:new.shuffle')}
              title={t('bots:new.shuffle')}
              onClick={() => setAvatar(randomBotAvatar())}
            >
              <BotAvatar avatar={avatar} size={56} />
              <span className="absolute -right-1 -bottom-1 grid size-5 place-items-center rounded-full border border-stroke bg-surface-raised text-content-muted group-hover:text-content">
                <Shuffle className="size-3" aria-hidden="true" />
              </span>
            </button>
            <FieldLabel variant="control" className="flex-1">
              {t('bots:new.name')}
              <Input
                value={name}
                autoFocus
                maxLength={24}
                placeholder={t('bots:new.namePlaceholder')}
                onChange={(event) => setName(event.target.value)}
              />
            </FieldLabel>
          </div>
          {choosingLook ? (
            <BotAvatarPicker value={avatar} onChange={setAvatar} />
          ) : (
            <button
              type="button"
              className="-mt-2 self-start text-xs text-content-muted underline-offset-2 hover:text-content hover:underline"
              onClick={() => setChoosingLook(true)}
            >
              {t('bots:new.chooseLook')}
            </button>
          )}
          <FieldLabel variant="control">
            {t('bots:new.duty')}
            <Textarea
              value={instructions}
              rows={3}
              maxLength={12_000}
              placeholder={t('bots:new.dutyPlaceholder')}
              onChange={(event) => setInstructions(event.target.value)}
            />
          </FieldLabel>
          {editingFolder ? (
            <FieldLabel variant="control">
              {t('bots:new.folder')}
              <span className="flex gap-2">
                <Input
                  value={cwd}
                  className="font-mono"
                  placeholder={t('bots:new.folderDefault')}
                  onChange={(event) => setCwd(event.target.value)}
                />
                <Button type="button" variant="outline" onClick={() => setPicking(true)}>
                  <FolderOpen aria-hidden="true" />
                  {t('bots:new.pick')}
                </Button>
              </span>
            </FieldLabel>
          ) : (
            <p className="text-xs text-content-muted">
              {t('bots:new.folderSummary')}{' '}
              <button
                type="button"
                className="text-content-secondary underline underline-offset-2 hover:text-content"
                onClick={() => setEditingFolder(true)}
              >
                {t('bots:new.changeFolder')}
              </button>
            </p>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              {t('bots:new.cancel')}
            </Button>
            <Button type="submit" disabled={saving || !name.trim()}>
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
