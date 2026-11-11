// Bot 设置页“它自己做”里的巡看与简报（默认关）：空闲时每隔几小时只读巡看一遍它负责的东西，发现写成对话里一条安静的建议
// （你说“做吧”它才派活）；每天定时写一份简报。两项各自开关。
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { SettingsRow } from '@/components/app/SettingsRow'
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
    <>
      <SettingsRow
        title={t('bots:proactive.check')}
        description={t('bots:proactive.checkHint', { name: bot.name })}
        control={
          <Switch
            checked={bot.proactive.enabled}
            disabled={saving}
            aria-label={t('bots:proactive.check')}
            onCheckedChange={(enabled) => onSave({ enabled })}
          />
        }
      />
      {bot.proactive.enabled && (
        <SettingsRow
          nested
          title={t('bots:proactive.every')}
          control={
            <AppSelect
              value={String(bot.proactive.everyHours)}
              className="w-48"
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
          }
        />
      )}
      <SettingsRow
        title={t('bots:proactive.brief')}
        description={t('bots:proactive.briefHint')}
        control={
          <Switch
            checked={bot.proactive.brief}
            disabled={saving}
            aria-label={t('bots:proactive.brief')}
            onCheckedChange={(brief) => onSave({ brief })}
          />
        }
      />
      {bot.proactive.brief && (
        <SettingsRow
          nested
          title={t('bots:proactive.briefAt')}
          control={
            <Input
              type="time"
              value={briefAt}
              className="w-48"
              aria-label={t('bots:proactive.briefAt')}
              disabled={saving}
              onChange={(event) => setBriefAt(event.target.value)}
              onBlur={() => {
                if (/^\d{2}:\d{2}$/.test(briefAt) && briefAt !== bot.proactive.briefAt)
                  onSave({ briefAt })
              }}
            />
          }
        />
      )}
    </>
  )
}
