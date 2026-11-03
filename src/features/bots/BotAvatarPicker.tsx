// 选 Bot 的样子：12 个角色、6 种颜色。新建时默认随机给一个（randomBotAvatar），想换时才展开这里。
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'
import { BOT_CHARACTERS, BOT_COLORS, type BotAvatarValue } from './avatar'
import { BotAvatar } from './BotAvatar'
import { characterLabel } from './bot-labels'

export function BotAvatarPicker({
  value,
  onChange,
  disabled = false,
}: {
  value: BotAvatarValue
  onChange: (value: BotAvatarValue) => void
  disabled?: boolean
}) {
  const { t } = useI18n()
  return (
    <div className="flex flex-col gap-2">
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
            disabled={disabled}
            aria-checked={value.character === character}
            aria-label={characterLabel(t, character)}
            title={characterLabel(t, character)}
            className={cn(
              'grid place-items-center rounded-[12px] p-0.5 ring-offset-2 ring-offset-surface-raised',
              value.character === character && 'ring-2 ring-content',
            )}
            onClick={() => onChange({ ...value, character })}
          >
            <BotAvatar avatar={{ ...value, character }} size={40} animate={false} />
          </button>
        ))}
      </div>
      <div role="radiogroup" aria-label={t('bots:new.color')} className="flex gap-2">
        {BOT_COLORS.map((color) => (
          <button
            key={color}
            type="button"
            role="radio"
            disabled={disabled}
            aria-checked={value.color === color}
            aria-label={t('bots:new.colorOption', { index: BOT_COLORS.indexOf(color) + 1 })}
            className={cn(
              'rounded-full p-0.5 ring-offset-2 ring-offset-surface-raised',
              value.color === color && 'ring-2 ring-content',
            )}
            onClick={() => onChange({ ...value, color })}
          >
            <BotAvatar
              avatar={{ ...value, color }}
              size={24}
              animate={false}
              className="rounded-full"
            />
          </button>
        ))}
      </div>
    </div>
  )
}
