// Bot 的设置（设置页 /bots/:id 的内容）：头像、名字和“它负责什么”在最上面；电脑、在聊天里叫它、定时、
// 高级（权限、工具、暂停）都收在折叠分区里，用默认值就能用，想改时再展开。和它说话在它的会话里。
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@/components/ui/accordion'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { BotAvatar } from './BotAvatar'
import { BotAvatarPicker } from './BotAvatarPicker'
import { BotComputerTab } from './BotComputerTab'
import { ChannelEditor, ScheduleEditor } from './BotTriggers'
import { updateBot, type Bot, type BotEnvironment } from './bots-api'
import { environmentLabel } from './bot-environments'
import { executionModeLabel } from './bot-labels'
import { BOT_READ_ONLY_TOOLS, botAvatarState, botToolsPreset } from './bot-presentation'
import { useBotsStore } from './bots-store'

export function BotDetails({
  bot,
  environments,
  onEnvironmentsChanged,
  onError,
}: {
  bot: Bot
  environments: BotEnvironment[]
  onEnvironmentsChanged: () => void
  onError: (message: string) => void
}) {
  const { t } = useI18n()
  const upsert = useBotsStore((state) => state.upsert)
  const [instructions, setInstructions] = useState(bot.instructions)
  const [saving, setSaving] = useState(false)
  const [choosingLook, setChoosingLook] = useState(false)
  useEffect(() => setInstructions(bot.instructions), [bot.id, bot.instructions])

  const save = async (patch: Parameters<typeof updateBot>[1]) => {
    setSaving(true)
    try {
      upsert(await updateBot(bot.id, patch))
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  const environment = environments.find((item) => item.id === bot.workspace.environmentId)
  const folder = bot.workspace.cwd.split(/[\\/]/).filter(Boolean).at(-1) || bot.workspace.cwd
  return (
    <div className="flex flex-col gap-4 pb-4">
      <div className="flex items-center gap-3">
        <BotAvatar avatar={bot.avatar} state={botAvatarState(bot)} size={48} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-base font-semibold text-content">{bot.name}</div>
          <button
            type="button"
            className="text-xs text-content-muted underline-offset-2 hover:text-content hover:underline"
            onClick={() => setChoosingLook((open) => !open)}
          >
            {choosingLook ? t('bots:settings.doneLook') : t('bots:settings.changeLook')}
          </button>
        </div>
      </div>
      {choosingLook && (
        <BotAvatarPicker
          value={bot.avatar}
          disabled={saving}
          onChange={(avatar) => void save({ avatar })}
        />
      )}
      <section aria-label={t('bots:details.instructions')}>
        <h3 className="mb-1 text-sm font-medium text-content">{t('bots:details.instructions')}</h3>
        <Textarea
          value={instructions}
          rows={4}
          maxLength={12_000}
          aria-label={t('bots:details.instructions')}
          placeholder={t('bots:details.instructionsPlaceholder')}
          onChange={(event) => setInstructions(event.target.value)}
        />
        {instructions !== bot.instructions && (
          <div className="mt-2 flex justify-end">
            <Button size="sm" disabled={saving} onClick={() => void save({ instructions })}>
              {t('bots:details.save')}
            </Button>
          </div>
        )}
      </section>
      {/* 分区内容是展开后才加载的（如定时列表），高度跟着内容走，不按展开那一刻固定。 */}
      <Accordion type="multiple" className="border-t border-stroke">
        <AccordionItem value="computer">
          <AccordionTrigger>
            <span className="flex min-w-0 flex-1 items-baseline gap-2">
              {t('bots:settings.computer')}
              <span className="truncate text-xs font-normal text-content-muted">
                {t('bots:settings.computerSummary', {
                  computer: environmentLabel(t, environment),
                  folder,
                })}
              </span>
            </span>
          </AccordionTrigger>
          <AccordionContent className="h-auto">
            <BotComputerTab
              bot={bot}
              environments={environments}
              onEnvironmentsChanged={onEnvironmentsChanged}
              onError={onError}
            />
          </AccordionContent>
        </AccordionItem>
        <AccordionItem value="chats">
          <AccordionTrigger>
            <span className="flex min-w-0 flex-1 items-baseline gap-2">
              {t('bots:triggers.chats')}
              <span className="truncate text-xs font-normal text-content-muted">
                {bot.channels.length
                  ? t('bots:settings.chatsCount', { count: bot.channels.length })
                  : t('bots:settings.notSet')}
              </span>
            </span>
          </AccordionTrigger>
          <AccordionContent className="h-auto">
            <ChannelEditor bot={bot} onError={onError} />
          </AccordionContent>
        </AccordionItem>
        <AccordionItem value="schedules">
          <AccordionTrigger>{t('bots:triggers.schedules')}</AccordionTrigger>
          <AccordionContent className="h-auto">
            <ScheduleEditor bot={bot} onError={onError} />
          </AccordionContent>
        </AccordionItem>
        <AccordionItem value="advanced">
          <AccordionTrigger>
            <span className="flex min-w-0 flex-1 items-baseline gap-2">
              {t('bots:settings.advanced')}
              <span className="truncate text-xs font-normal text-content-muted">
                {t('bots:settings.advancedSummary')}
              </span>
            </span>
          </AccordionTrigger>
          <AccordionContent className="flex h-auto flex-col gap-3">
            <label className="flex items-center justify-between gap-3 text-sm">
              <span>{t('bots:details.acceptsWork')}</span>
              <Switch
                checked={bot.enabled}
                disabled={saving}
                onCheckedChange={(enabled) => void save({ enabled })}
              />
            </label>
            <label className="flex items-center justify-between gap-3 text-sm">
              <span className="flex-none">{t('bots:details.permission')}</span>
              <AppSelect
                value={bot.executionMode}
                className="w-44"
                aria-label={t('bots:details.permission')}
                disabled={saving}
                onChange={(event) =>
                  void save({ executionMode: event.target.value as Bot['executionMode'] })
                }
              >
                {(['approval-required', 'workspace-write', 'full-access'] as const).map((mode) => (
                  <option key={mode} value={mode}>
                    {executionModeLabel(t, mode)}
                  </option>
                ))}
              </AppSelect>
            </label>
            <label className="flex items-center justify-between gap-3 text-sm">
              <span className="flex-none">{t('bots:details.tools')}</span>
              <AppSelect
                value={botToolsPreset(bot.tools)}
                className="w-44"
                aria-label={t('bots:details.tools')}
                disabled={saving}
                onChange={(event) =>
                  void save({
                    tools: event.target.value === 'read-only' ? [...BOT_READ_ONLY_TOOLS] : null,
                  })
                }
              >
                <option value="all">{t('bots:details.toolsAll')}</option>
                <option value="read-only">{t('bots:details.toolsReadOnly')}</option>
                {/* 经接口设置的自定义名单只显示，不在这里编辑。 */}
                {botToolsPreset(bot.tools) === 'custom' && (
                  <option value="custom" disabled>
                    {t('bots:details.toolsCustom', { count: bot.tools?.length ?? 0 })}
                  </option>
                )}
              </AppSelect>
            </label>
            <p className="text-xs text-content-muted">{t('bots:details.toolsHint')}</p>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </div>
  )
}
