// 设置里的「指令」分区：在聊天里点名后发序号或名字就能用的事。就地编辑，不弹窗。
// - 交给 Bot 做：把写好的事发进它的对话；可以允许带一句补充（别人带的只当他的说法）。
// - 固定回复：直接发写好的文字，不调用模型、不花用量。
// - 谁能用：仅管理员，或所有人（聊天规则里打开了“其他人也能用公开指令”的聊天才生效）。
import { useState } from 'react'
import { ArrowDown, ArrowUp } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { updateBot, type Bot, type BotCommand } from './bots-api'
import { useBotsStore } from './bots-store'

const EMPTY: BotCommand = {
  name: '',
  kind: 'task',
  text: '',
  audience: 'admins',
  allowExtra: false,
}

function CommandForm({
  initial,
  saving,
  onCancel,
  onSave,
}: {
  initial: BotCommand
  saving: boolean
  onCancel: () => void
  onSave: (command: BotCommand) => void
}) {
  const { t } = useI18n()
  const [draft, setDraft] = useState(initial)
  const set = (patch: Partial<BotCommand>) => setDraft((current) => ({ ...current, ...patch }))
  const task = draft.kind === 'task'
  return (
    <form
      data-bot-command-form
      className="flex flex-col gap-2 rounded-lg border border-stroke p-2.5"
      onSubmit={(event) => {
        event.preventDefault()
        onSave({ ...draft, name: draft.name.trim(), text: draft.text.trim() })
      }}
    >
      <Input
        value={draft.name}
        autoFocus
        maxLength={24}
        aria-label={t('bots:commands.name')}
        placeholder={t('bots:commands.namePlaceholder')}
        onChange={(event) => set({ name: event.target.value })}
      />
      <SegmentedControl
        value={draft.kind}
        onValueChange={(kind) => set({ kind, allowExtra: kind === 'task' && draft.allowExtra })}
        options={[
          { value: 'task', label: t('bots:commands.task') },
          { value: 'reply', label: t('bots:commands.reply') },
        ]}
      />
      <Textarea
        value={draft.text}
        rows={3}
        maxLength={4000}
        aria-label={task ? t('bots:commands.taskText') : t('bots:commands.replyText')}
        placeholder={
          task ? t('bots:commands.taskPlaceholder') : t('bots:commands.replyPlaceholder')
        }
        onChange={(event) => set({ text: event.target.value })}
      />
      <div className="flex flex-wrap items-center justify-between gap-2 text-sm">
        <span className="text-content-secondary">{t('bots:commands.audience')}</span>
        <SegmentedControl
          value={draft.audience}
          onValueChange={(audience) => set({ audience })}
          options={[
            { value: 'admins', label: t('bots:commands.admins') },
            { value: 'everyone', label: t('bots:commands.everyone') },
          ]}
        />
      </div>
      {task && (
        <label className="flex items-center justify-between gap-3 text-sm">
          <span className="text-content-secondary">{t('bots:commands.allowExtra')}</span>
          <Switch
            checked={draft.allowExtra}
            onCheckedChange={(allowExtra) => set({ allowExtra })}
          />
        </label>
      )}
      {task && draft.allowExtra && draft.audience === 'everyone' && (
        <p className="m-0 text-xs text-content-muted">{t('bots:commands.extraNote')}</p>
      )}
      <div className="flex justify-end gap-2">
        <Button type="button" size="sm" variant="ghost" onClick={onCancel}>
          {t('bots:triggers.cancel')}
        </Button>
        <Button
          type="submit"
          size="sm"
          disabled={saving || !draft.name.trim() || !draft.text.trim()}
        >
          {t('bots:commands.save')}
        </Button>
      </div>
    </form>
  )
}

export function BotCommandsSection({
  bot,
  onError,
}: {
  bot: Bot
  onError: (message: string) => void
}) {
  const { t } = useI18n()
  const upsert = useBotsStore((state) => state.upsert)
  // null：没在编辑；-1：新加一条；其他：正在改第几条。
  const [editing, setEditing] = useState<number | null>(null)
  const [saving, setSaving] = useState(false)
  const save = async (commands: BotCommand[]) => {
    setSaving(true)
    try {
      upsert(await updateBot(bot.id, { commands }))
      return true
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
      return false
    } finally {
      setSaving(false)
    }
  }
  const move = (index: number, offset: number) => {
    const next = [...bot.commands]
    const [item] = next.splice(index, 1)
    next.splice(index + offset, 0, item)
    void save(next)
  }
  const anyoneCanUse = bot.channels.some((channel) => channel.everyone)
  return (
    <div className="flex flex-col">
      <p className="mb-2 text-xs text-content-muted">
        {t('bots:commands.hint', { name: bot.name })}
      </p>
      {bot.commands.some((command) => command.audience === 'everyone') && !anyoneCanUse && (
        <p className="mb-2 text-xs text-content-muted">{t('bots:commands.noEveryoneChat')}</p>
      )}
      {bot.commands.length > 0 && (
        <ol className="m-0 mb-2 flex list-none flex-col gap-1.5 p-0">
          {bot.commands.map((command, index) =>
            editing === index ? (
              <li key={command.name}>
                <CommandForm
                  initial={command}
                  saving={saving}
                  onCancel={() => setEditing(null)}
                  onSave={async (next) => {
                    if (
                      await save(
                        bot.commands.map((item, position) => (position === index ? next : item)),
                      )
                    )
                      setEditing(null)
                  }}
                />
              </li>
            ) : (
              <li
                key={command.name}
                data-bot-command={command.name}
                className="flex items-start gap-2 rounded-lg bg-surface-subtle px-2.5 py-2 text-xs"
              >
                <span className="w-4 flex-none pt-px text-right font-mono text-content-muted">
                  {index + 1}
                </span>
                <span className="min-w-0 flex-1">
                  <b className="block truncate font-medium text-content">{command.name}</b>
                  <span className="block truncate text-content-muted">
                    {[
                      command.audience === 'everyone'
                        ? t('bots:commands.everyone')
                        : t('bots:commands.admins'),
                      command.kind === 'reply' ? t('bots:commands.reply') : t('bots:commands.task'),
                      command.allowExtra ? t('bots:commands.extraAllowed') : '',
                      command.text,
                    ]
                      .filter(Boolean)
                      .join(' · ')}
                  </span>
                </span>
                <Button
                  size="icon-sm"
                  variant="ghost"
                  aria-label={t('bots:commands.moveUp')}
                  disabled={saving || editing !== null || index === 0}
                  onClick={() => move(index, -1)}
                >
                  <ArrowUp />
                </Button>
                <Button
                  size="icon-sm"
                  variant="ghost"
                  aria-label={t('bots:commands.moveDown')}
                  disabled={saving || editing !== null || index === bot.commands.length - 1}
                  onClick={() => move(index, 1)}
                >
                  <ArrowDown />
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={saving || editing !== null}
                  onClick={() => setEditing(index)}
                >
                  {t('bots:commands.edit')}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={saving || editing !== null}
                  onClick={() =>
                    void save(bot.commands.filter((_, position) => position !== index))
                  }
                >
                  {t('bots:triggers.remove')}
                </Button>
              </li>
            ),
          )}
        </ol>
      )}
      {editing === -1 ? (
        <CommandForm
          initial={EMPTY}
          saving={saving}
          onCancel={() => setEditing(null)}
          onSave={async (command) => {
            if (await save([...bot.commands, command])) setEditing(null)
          }}
        />
      ) : (
        <Button
          size="sm"
          variant="outline"
          className="self-start"
          disabled={editing !== null || bot.commands.length >= 20}
          onClick={() => setEditing(-1)}
        >
          {t('bots:commands.add')}
        </Button>
      )}
    </div>
  )
}
