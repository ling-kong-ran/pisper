// Bot 设置页的“它学会的”分区：它做完活复盘时自己写的技能。能展开看全文、删掉，或设成定时
// （“每周五照这个做一遍”，就是一条发给它的定时）。技能只由复盘写入，这里不手写。
import { useEffect, useState } from 'react'
import { CalendarClock, ChevronDown, ChevronRight, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { ScheduleForm } from './BotTriggers'
import {
  deleteBotSkill,
  fetchBotSkill,
  fetchBotSkills,
  type Bot,
  type BotSkillSummary,
} from './bots-api'

export function BotSkillsSection({
  bot,
  onError,
  onCount,
}: {
  bot: Bot
  onError: (message: string) => void
  onCount?: (count: number) => void
}) {
  const { t } = useI18n()
  const [skills, setSkills] = useState<BotSkillSummary[] | null>(null)
  const [open, setOpen] = useState<{ name: string; body: string | null } | null>(null)
  const [scheduling, setScheduling] = useState('')
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    const controller = new AbortController()
    fetchBotSkills(bot.id, controller.signal)
      .then((next) => setSkills(next))
      .catch((error: unknown) => {
        if (!controller.signal.aborted) {
          setSkills([])
          onError(error instanceof Error ? error.message : String(error))
        }
      })
    return () => controller.abort()
  }, [bot.id, onError])
  useEffect(() => {
    if (skills) onCount?.(skills.length)
  }, [skills, onCount])
  const toggle = (name: string) => {
    if (open?.name === name) {
      setOpen(null)
      return
    }
    setOpen({ name, body: null })
    fetchBotSkill(bot.id, name)
      .then((skill) =>
        setOpen((current) => (current?.name === name ? { name, body: skill.body } : current)),
      )
      .catch((error: unknown) => {
        setOpen(null)
        onError(error instanceof Error ? error.message : String(error))
      })
  }
  const remove = async (name: string) => {
    setBusy(true)
    try {
      await deleteBotSkill(bot.id, name)
      setSkills((current) => (current || []).filter((skill) => skill.name !== name))
      if (open?.name === name) setOpen(null)
    } catch (error) {
      onError(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="flex flex-col gap-2">
      <p className="m-0 text-xs text-content-muted">{t('bots:skills.hint', { name: bot.name })}</p>
      {skills && !skills.length && (
        <InlineEmptyState compact title={t('bots:skills.empty', { name: bot.name })} />
      )}
      {skills && skills.length > 0 && (
        <ul role="list" className="m-0 flex list-none flex-col gap-1.5 p-0">
          {skills.map((skill) => {
            const expanded = open?.name === skill.name
            return (
              <li
                key={skill.name}
                data-bot-skill={skill.name}
                className="flex flex-col gap-2 rounded-lg bg-surface-subtle px-2.5 py-2 text-[13px]"
              >
                <div className="flex items-start gap-2">
                  <button
                    type="button"
                    className="flex min-w-0 flex-1 items-start gap-1.5 text-left"
                    aria-expanded={expanded}
                    onClick={() => toggle(skill.name)}
                  >
                    {expanded ? (
                      <ChevronDown aria-hidden="true" className="mt-0.5 size-3.5 flex-none" />
                    ) : (
                      <ChevronRight aria-hidden="true" className="mt-0.5 size-3.5 flex-none" />
                    )}
                    <span className="min-w-0">
                      <b className="block truncate font-medium text-content">{skill.name}</b>
                      <span className="block text-xs text-content-muted">{skill.description}</span>
                    </span>
                  </button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-xs"
                    className="flex-none pointer-coarse:size-11"
                    aria-label={t('bots:skills.schedule')}
                    title={t('bots:skills.schedule')}
                    disabled={busy}
                    onClick={() => setScheduling((name) => (name === skill.name ? '' : skill.name))}
                  >
                    <CalendarClock aria-hidden="true" />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-xs"
                    className="flex-none pointer-coarse:size-11"
                    aria-label={t('bots:skills.delete')}
                    title={t('bots:skills.delete')}
                    disabled={busy}
                    onClick={() => void remove(skill.name)}
                  >
                    <Trash2 aria-hidden="true" />
                  </Button>
                </div>
                {expanded && (
                  <pre className="m-0 max-h-72 overflow-auto rounded-md bg-surface px-2.5 py-2 text-xs whitespace-pre-wrap text-content">
                    {open.body ?? t('bots:skills.loading')}
                  </pre>
                )}
                {scheduling === skill.name && (
                  <ScheduleForm
                    bot={bot}
                    initialMessage={t('bots:skills.scheduleMessage', { name: skill.name })}
                    onCancel={() => setScheduling('')}
                    onCreated={() => setScheduling('')}
                    onError={onError}
                  />
                )}
              </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}
