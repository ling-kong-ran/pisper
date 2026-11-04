// Bot 设置页的“主动”分区（默认关）：空闲时每隔几小时只读巡看一遍它负责的东西，发现写成对话里一条安静的建议
// （你说“做吧”它才派活）；每天定时写一份简报。两项各自开关。
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { BOT_PROACTIVE_HOURS, type Bot, type BotProactive } from './bots-api'

export function BotProactiveSection({
  bot,
  saving,
  onSave,
}: {
  bot: Bot
  saving: boolean
  onSave: (proactive: Partial<BotProactive>) => void
}) {
  const { t } = useI18n()
  const [briefAt, setBriefAt] = useState(bot.proactive.briefAt)
  useEffect(() => setBriefAt(bot.proactive.briefAt), [bot.id, bot.proactive.briefAt])
  return (
    <div className="flex flex-col gap-3">
      <label className="flex items-center justify-between gap-3 text-sm">
        <span>{t('bots:proactive.check')}</span>
        <Switch
          checked={bot.proactive.enabled}
          disabled={saving}
          onCheckedChange={(enabled) => onSave({ enabled })}
        />
      </label>
      {bot.proactive.enabled && (
        <label className="flex items-center justify-between gap-3 text-sm">
          <span className="flex-none">{t('bots:proactive.every')}</span>
          <AppSelect
            value={String(bot.proactive.everyHours)}
            className="w-44"
            aria-label={t('bots:proactive.every')}
            disabled={saving}
            onChange={(event) =>
              onSave({ everyHours: Number(event.target.value) as BotProactive['everyHours'] })
            }
          >
            {BOT_PROACTIVE_HOURS.map((hours) => (
              <option key={hours} value={hours}>
                {t('bots:proactive.everyHours', { count: hours })}
              </option>
            ))}
          </AppSelect>
        </label>
      )}
      <p className="m-0 text-xs text-content-muted">
        {t('bots:proactive.checkHint', { name: bot.name })}
      </p>
      <label className="flex items-center justify-between gap-3 text-sm">
        <span>{t('bots:proactive.brief')}</span>
        <Switch
          checked={bot.proactive.brief}
          disabled={saving}
          onCheckedChange={(brief) => onSave({ brief })}
        />
      </label>
      {bot.proactive.brief && (
        <label className="flex items-center justify-between gap-3 text-sm">
          <span className="flex-none">{t('bots:proactive.briefAt')}</span>
          <Input
            type="time"
            value={briefAt}
            className="w-44"
            aria-label={t('bots:proactive.briefAt')}
            disabled={saving}
            onChange={(event) => setBriefAt(event.target.value)}
            onBlur={() => {
              if (/^\d{2}:\d{2}$/.test(briefAt) && briefAt !== bot.proactive.briefAt)
                onSave({ briefAt })
            }}
          />
        </label>
      )}
      <p className="m-0 text-xs text-content-muted">{t('bots:proactive.briefHint')}</p>
    </div>
  )
}
