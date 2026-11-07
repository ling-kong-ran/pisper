// Bot 的设置（设置页 /bots/:id 的内容）：头像、名字和“它负责什么”在最上面；电脑、在聊天里叫它、定时、
// 高级（权限、工具、暂停）都收在折叠分区里，用默认值就能用，想改时再展开。和它说话在它的会话里。
import { useEffect, useRef, useState } from 'react'
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
import { BotModelSection } from './BotModelSection'
import { BotMemorySection } from './BotMemorySection'
import { BotSkillsSection } from './BotSkillsSection'
import { BotProactiveSection } from './BotProactiveSection'
import { BotCommandsSection } from './BotCommandsSection'
import { ChannelEditor, ScheduleEditor } from './BotTriggers'
import { updateBot, type Bot, type BotEnvironment } from './bots-api'
import { environmentLabel } from './bot-environments'
import { botModelLabel, botThinkingLabel, executionModeLabel } from './bot-labels'
import {
  BOT_READ_ONLY_TOOLS,
  botAvatarState,
  botFolderName,
  botToolsPreset,
} from './bot-presentation'
import { useBotsStore } from './bots-store'

export function BotDetails({
  bot,
  environments,
  openSection,
  onEnvironmentsChanged,
  onError,
}: {
  bot: Bot
  openSection?: string
  environments: BotEnvironment[]
  onEnvironmentsChanged: () => void
  onError: (message: string) => void
}) {
  const { t } = useI18n()
  const upsert = useBotsStore((state) => state.upsert)
  const [instructions, setInstructions] = useState(bot.instructions)
  const [saving, setSaving] = useState(false)
  const [choosingLook, setChoosingLook] = useState(false)
  const [memoryCount, setMemoryCount] = useState<number | null>(null)
  const [skillCount, setSkillCount] = useState<number | null>(null)
  useEffect(() => setInstructions(bot.instructions), [bot.id, bot.instructions])
  // 从别处跳到某个分区时（招呼里的聊天接入）：它可能在屏幕下面，滚到能看见。
  const rootRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (openSection) rootRef.current?.querySelector('[data-state="open"]')?.scrollIntoView()
  }, [openSection])

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
  const folder = botFolderName(bot.workspace.cwd)
  return (
    <div ref={rootRef} className="flex flex-col gap-4 pb-4">
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
      <Accordion
        type="multiple"
        defaultValue={openSection ? [openSection] : undefined}
        className="border-t border-stroke"
      >
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
        {/* 记忆：它自己记得的事。展开时才加载。 */}
        <AccordionItem value="memory">
          <AccordionTrigger>
            <span className="flex min-w-0 flex-1 items-baseline gap-2">
              {t('bots:memory.title')}
              {memoryCount !== null && (
                <span className="truncate text-xs font-normal text-content-muted">
                  {t('bots:memory.count', { count: memoryCount })}
                </span>
              )}
            </span>
          </AccordionTrigger>
          <AccordionContent className="h-auto">
            <BotMemorySection
              bot={bot}
              saving={saving}
              onError={onError}
              onCount={setMemoryCount}
              onReadGlobalChange={(readGlobalMemory) => void save({ readGlobalMemory })}
            />
          </AccordionContent>
        </AccordionItem>
        {/* 它学会的：做完活复盘时自己写的技能。展开时才加载。 */}
        <AccordionItem value="skills">
          <AccordionTrigger>
            <span className="flex min-w-0 flex-1 items-baseline gap-2">
              {t('bots:skills.title')}
              {skillCount !== null && (
                <span className="truncate text-xs font-normal text-content-muted">
                  {t('bots:skills.count', { count: skillCount })}
                </span>
              )}
            </span>
          </AccordionTrigger>
          <AccordionContent className="h-auto">
            <BotSkillsSection bot={bot} onError={onError} onCount={setSkillCount} />
          </AccordionContent>
        </AccordionItem>
        {/* 主动（默认关）：空闲时只读巡看、每日简报。 */}
        <AccordionItem value="proactive">
          <AccordionTrigger>
            <span className="flex min-w-0 flex-1 items-baseline gap-2">
              {t('bots:proactive.title')}
              <span className="truncate text-xs font-normal text-content-muted">
                {bot.proactive.enabled || bot.proactive.brief
                  ? [
                      bot.proactive.enabled
                        ? t('bots:proactive.summaryCheck', { count: bot.proactive.everyHours })
                        : '',
                      bot.proactive.brief
                        ? t('bots:proactive.summaryBrief', { time: bot.proactive.briefAt })
                        : '',
                    ]
                      .filter(Boolean)
                      .join(' · ')
                  : t('bots:proactive.off')}
              </span>
            </span>
          </AccordionTrigger>
          <AccordionContent className="h-auto">
            <BotProactiveSection
              bot={bot}
              saving={saving}
              onSave={(proactive) => void save({ proactive })}
            />
          </AccordionContent>
        </AccordionItem>
        <AccordionItem value="model">
          <AccordionTrigger>
            <span className="flex min-w-0 flex-1 items-baseline gap-2">
              {t('bots:model.title')}
              <span className="truncate text-xs font-normal text-content-muted">
                {t('bots:model.summary', {
                  model: botModelLabel(t, bot.model),
                  thinking: botThinkingLabel(t, bot.thinking),
                })}
              </span>
            </span>
          </AccordionTrigger>
          <AccordionContent className="flex h-auto flex-col gap-3">
            <BotModelSection bot={bot} saving={saving} onSave={(patch) => void save(patch)} />
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
        {/* 指令：在聊天里点名后回序号就能用；公开的指令其他人也能用。 */}
        <AccordionItem value="commands">
          <AccordionTrigger>
            <span className="flex min-w-0 flex-1 items-baseline gap-2">
              {t('bots:commands.title')}
              <span className="truncate text-xs font-normal text-content-muted">
                {bot.commands.length
                  ? t('bots:commands.count', {
                      count: bot.commands.length,
                      open: bot.commands.filter((command) => command.audience === 'everyone')
                        .length,
                    })
                  : t('bots:settings.notSet')}
              </span>
            </span>
          </AccordionTrigger>
          <AccordionContent className="h-auto">
            <BotCommandsSection bot={bot} onError={onError} />
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
                {t('bots:settings.advancedSummary', {
                  permission: executionModeLabel(t, bot.executionMode),
                })}
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
            <p className="text-xs text-content-muted">{t('bots:details.permissionHint')}</p>
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
            <label className="flex items-center justify-between gap-3 text-sm">
              <span className="flex-none">{t('bots:details.parallel')}</span>
              <AppSelect
                value={String(bot.maxParallelAgents)}
                className="w-44"
                aria-label={t('bots:details.parallel')}
                disabled={saving}
                onChange={(event) => void save({ maxParallelAgents: Number(event.target.value) })}
              >
                {[1, 2, 3, 4, 5, 6].map((count) => (
                  <option key={count} value={count}>
                    {t('bots:details.parallelCount', { count })}
                  </option>
                ))}
              </AppSelect>
            </label>
            <p className="text-xs text-content-muted">{t('bots:details.parallelHint')}</p>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </div>
  )
}
