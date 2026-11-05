// Bot 设置页的“记忆”分区：它自己记得的事，一条一条地看、改、删，也能手动加一条。
// 对话里说“记住…”“以后别…”、做完复盘也会写进来，直接生效。上面的开关决定它能不能只读用户的全局记忆。
import { useEffect, useState } from 'react'
import { Pencil, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import {
  addBotMemory,
  deleteBotMemory,
  editBotMemory,
  fetchBotMemory,
  type Bot,
  type BotMemoryEntry,
} from './bots-api'

export function BotMemorySection({
  bot,
  saving = false,
  onError,
  onCount,
  onReadGlobalChange,
}: {
  bot: Bot
  saving?: boolean
  onError: (message: string) => void
  onCount?: (count: number) => void
  onReadGlobalChange?: (readGlobalMemory: boolean) => void
}) {
  const { t } = useI18n()
  const [entries, setEntries] = useState<BotMemoryEntry[] | null>(null)
  const [draft, setDraft] = useState('')
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null)
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    const controller = new AbortController()
    fetchBotMemory(bot.id, controller.signal)
      .then((next) => setEntries(next))
      .catch((error: unknown) => {
        if (!controller.signal.aborted) {
          setEntries([])
          onError(error instanceof Error ? error.message : String(error))
        }
      })
    return () => controller.abort()
  }, [bot.id, onError])
  useEffect(() => {
    if (entries) onCount?.(entries.length)
  }, [entries, onCount])
  const run = async (action: () => Promise<void>) => {
    setBusy(true)
    try {
      await action()
    } catch (error) {
      onError(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(false)
    }
  }
  const add = () =>
    run(async () => {
      const entry = await addBotMemory(bot.id, draft)
      setEntries((current) => [...(current || []).filter((item) => item.id !== entry.id), entry])
      setDraft('')
    })
  const save = () =>
    run(async () => {
      if (!editing) return
      const entry = await editBotMemory(bot.id, editing.id, editing.text)
      setEntries((current) => (current || []).map((item) => (item.id === entry.id ? entry : item)))
      setEditing(null)
    })
  const remove = (id: string) =>
    run(async () => {
      await deleteBotMemory(bot.id, id)
      setEntries((current) => (current || []).filter((item) => item.id !== id))
    })
  return (
    <div className="flex flex-col gap-2">
      <p className="m-0 text-xs text-content-muted">{t('bots:memory.hint', { name: bot.name })}</p>
      {onReadGlobalChange && (
        <label className="flex items-center justify-between gap-3 text-sm">
          <span className="flex min-w-0 flex-col gap-0.5">
            <span>{t('bots:memory.readGlobal')}</span>
            <span className="text-xs text-content-muted">{t('bots:memory.readGlobalHint')}</span>
          </span>
          <Switch
            checked={bot.readGlobalMemory}
            disabled={saving}
            onCheckedChange={onReadGlobalChange}
          />
        </label>
      )}
      {entries && !entries.length && (
        <InlineEmptyState compact title={t('bots:memory.empty', { name: bot.name })} />
      )}
      {entries && entries.length > 0 && (
        <ul role="list" className="m-0 flex list-none flex-col gap-1.5 p-0">
          {[...entries].reverse().map((entry) =>
            editing?.id === entry.id ? (
              <li key={entry.id} className="flex items-center gap-2">
                <Input
                  value={editing.text}
                  maxLength={500}
                  aria-label={t('bots:memory.editLabel')}
                  onChange={(event) => setEditing({ id: entry.id, text: event.target.value })}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' && editing.text.trim()) void save()
                    if (event.key === 'Escape') setEditing(null)
                  }}
                />
                <Button
                  size="sm"
                  disabled={busy || !editing.text.trim()}
                  onClick={() => void save()}
                >
                  {t('bots:memory.save')}
                </Button>
                <Button size="sm" variant="ghost" onClick={() => setEditing(null)}>
                  {t('bots:memory.cancel')}
                </Button>
              </li>
            ) : (
              <li
                key={entry.id}
                data-bot-memory={entry.id}
                className="group flex items-start gap-2 rounded-lg bg-surface-subtle px-2.5 py-2 text-[13px]"
              >
                <span className="min-w-0 flex-1 text-content">{entry.text}</span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  className="flex-none pointer-coarse:size-11"
                  aria-label={t('bots:memory.edit')}
                  title={t('bots:memory.edit')}
                  disabled={busy}
                  onClick={() => setEditing({ id: entry.id, text: entry.text })}
                >
                  <Pencil aria-hidden="true" />
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  className="flex-none pointer-coarse:size-11"
                  aria-label={t('bots:memory.delete')}
                  title={t('bots:memory.delete')}
                  disabled={busy}
                  onClick={() => void remove(entry.id)}
                >
                  <Trash2 aria-hidden="true" />
                </Button>
              </li>
            ),
          )}
        </ul>
      )}
      <div className="flex items-center gap-2">
        <Input
          value={draft}
          maxLength={500}
          aria-label={t('bots:memory.addLabel')}
          placeholder={t('bots:memory.addPlaceholder')}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && draft.trim()) void add()
          }}
        />
        <Button
          size="sm"
          variant="outline"
          disabled={busy || !draft.trim()}
          onClick={() => void add()}
        >
          {t('bots:memory.add')}
        </Button>
      </div>
    </div>
  )
}
