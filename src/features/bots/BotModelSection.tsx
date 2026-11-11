// Bot 设置页“工作方式”里的模型与思考两行：用哪个模型、思考多深。Bot 的会话输入框不放模型芯片，在这里定，
// 改了就套到它的会话上（下一轮生效）。档位跟着“子代理模型档位”的设置走，也可以指定具体模型。
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { SettingsRow } from '@/components/app/SettingsRow'
import { compactModelName } from '@/lib/model-display'
import {
  BOT_MODEL_TIERS,
  BOT_THINKING_LEVELS,
  fetchBotModelOptions,
  type Bot,
  type BotModel,
  type BotModelOption,
  type BotThinking,
} from './bots-api'
import { botModelLabel, botThinkingLabel } from './bot-labels'

function modelValue(model: BotModel) {
  return 'ref' in model ? `ref:${model.ref}` : `tier:${model.tier}`
}

function parseModelValue(value: string): BotModel {
  if (value.startsWith('ref:')) return { ref: value.slice(4) }
  const tier = BOT_MODEL_TIERS.find((item) => `tier:${item}` === value)
  return { tier: tier ?? 'standard' }
}

export function BotModelSection({
  bot,
  saving,
  onSave,
}: {
  bot: Bot
  saving: boolean
  onSave: (patch: { model?: BotModel; thinking?: BotThinking | null }) => void
}) {
  const { t } = useI18n()
  const [options, setOptions] = useState<BotModelOption[]>([])
  useEffect(() => {
    let active = true
    // 读不到模型列表时只剩档位可选，不挡住设置页。
    fetchBotModelOptions()
      .then((next) => active && setOptions(next))
      .catch(() => active && setOptions([]))
    return () => {
      active = false
    }
  }, [])
  const names = options.map((option) => option.name)
  const current = modelValue(bot.model)
  return (
    <>
      <SettingsRow
        title={t('bots:model.model')}
        description={t('bots:model.hint')}
        control={
          <AppSelect
            value={current}
            className="w-48"
            aria-label={t('bots:model.model')}
            disabled={saving}
            onChange={(event) => onSave({ model: parseModelValue(event.target.value) })}
          >
            {BOT_MODEL_TIERS.map((tier) => (
              <option key={tier} value={`tier:${tier}`}>
                {botModelLabel(t, { tier })}
              </option>
            ))}
            {/* 已保存但现在不可用的模型也显示出来，便于看见并换掉。 */}
            {'ref' in bot.model && !options.some((option) => `ref:${option.ref}` === current) && (
              <option value={current}>{bot.model.ref}</option>
            )}
            {options.map((option) => (
              <option key={option.ref} value={`ref:${option.ref}`}>
                {`${compactModelName(option.name, names)} · ${option.providerName}`}
              </option>
            ))}
          </AppSelect>
        }
      />
      <SettingsRow
        title={t('bots:model.thinking')}
        control={
          <AppSelect
            value={bot.thinking ?? ''}
            className="w-48"
            aria-label={t('bots:model.thinking')}
            disabled={saving}
            onChange={(event) =>
              onSave({
                thinking: BOT_THINKING_LEVELS.find((level) => level === event.target.value) ?? null,
              })
            }
          >
            <option value="">{t('bots:model.thinkingDefault')}</option>
            {BOT_THINKING_LEVELS.map((level) => (
              <option key={level} value={level}>
                {botThinkingLabel(t, level)}
              </option>
            ))}
          </AppSelect>
        }
      />
    </>
  )
}
