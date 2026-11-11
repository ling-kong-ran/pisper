// Bot 的设置（设置页 /bots/:id 的内容）：头部是头像（点了换样子）、名字（就地改名）、现在的状态和“接活”开关，
// 下面是“它负责什么”，其余按“工作方式 / 谁能叫它 / 它自己做 / 它记得的”分组，每项一行，和设置页同一套行。
// 简单的选项直接在行里改；列表类（电脑、聊天、指令、定时、记忆、技能）点“管理”在行下面就地展开。
import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react'
import { ChevronRight, Pencil } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { SettingsRow, SettingsSection } from '@/components/app/SettingsRow'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import { BotAvatarPicker } from './BotAvatarPicker'
import { BotComputerTab } from './BotComputerTab'
import { BotModelSection } from './BotModelSection'
import { BotMemorySection } from './BotMemorySection'
import { BotSkillsSection } from './BotSkillsSection'
import { BotProactiveSection } from './BotProactiveSection'
import { BotCommandsSection } from './BotCommandsSection'
import { ChannelEditor, ScheduleEditor } from './BotTriggers'
import {
  fetchBotMemory,
  fetchBotSchedules,
  fetchBotSkills,
  updateBot,
  type Bot,
  type BotEnvironment,
} from './bots-api'
import { environmentLabel } from './bot-environments'
import { botModelLabel, executionModeLabel } from './bot-labels'
import {
  BOT_READ_ONLY_TOOLS,
  botActivityLine,
  botAvatarState,
  botFolderName,
  botToolsPreset,
} from './bot-presentation'
import { useBotsStore } from './bots-store'

// 列表类设置的一行：右边“管理”展开、“收起”收回，内容在行下面。
function ManagedRow({
  id,
  title,
  summary,
  open,
  onToggle,
  children,
}: {
  id: string
  title: ReactNode
  summary: ReactNode
  open: boolean
  onToggle: () => void
  children: ReactNode
}) {
  const { t } = useI18n()
  const contentId = useId()
  return (
    <SettingsRow
      data-bot-setting={id}
      data-state={open ? 'open' : 'closed'}
      title={title}
      description={summary}
      control={
        <Button
          variant="ghost"
          size="sm"
          aria-expanded={open}
          aria-controls={open ? contentId : undefined}
          onClick={onToggle}
        >
          {open ? t('bots:settings.collapse') : t('bots:settings.manage')}
          <ChevronRight
            aria-hidden="true"
            className={cn(
              'transition-transform motion-reduce:transition-none',
              open && 'rotate-90',
            )}
          />
        </Button>
      }
    >
      {open && (
        <div id={contentId} className="mt-3">
          {children}
        </div>
      )}
    </SettingsRow>
  )
}

// 头部：头像（点了在下面选样子）、名字（铅笔就地改名）、状态行、“接活”开关。
function BotHero({
  bot,
  environment,
  saving,
  onSave,
}: {
  bot: Bot
  environment: BotEnvironment | undefined
  saving: boolean
  onSave: (patch: Parameters<typeof updateBot>[1]) => Promise<boolean>
}) {
  const { t } = useI18n()
  const [choosingLook, setChoosingLook] = useState(false)
  const [renaming, setRenaming] = useState<string | null>(null)
  const rename = async () => {
    const name = renaming?.trim() ?? ''
    if (!name || name === bot.name) {
      setRenaming(null)
      return
    }
    if (await onSave({ name })) setRenaming(null)
  }
  const status = [
    botActivityLine(t, bot),
    t('bots:settings.computerSummary', {
      computer: environmentLabel(t, environment),
      folder: botFolderName(bot.workspace.cwd),
    }),
    botModelLabel(t, bot.model),
  ].join(' · ')
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-4">
        <button
          type="button"
          aria-label={t('bots:settings.changeLook')}
          aria-expanded={choosingLook}
          title={t('bots:settings.changeLook')}
          className="relative flex-none rounded-[16px] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
          onClick={() => setChoosingLook((open) => !open)}
        >
          <BotAvatar avatar={bot.avatar} state={botAvatarState(bot)} size={56} />
          <span className="absolute -right-1 -bottom-1 grid size-6 place-items-center rounded-full border border-stroke bg-solid text-content-muted shadow-edge">
            <Pencil className="size-3" aria-hidden="true" />
          </span>
        </button>
        <div className="min-w-0 flex-1">
          {renaming !== null ? (
            <form
              className="flex items-center gap-2"
              onSubmit={(event) => {
                event.preventDefault()
                void rename()
              }}
            >
              <Input
                autoFocus
                value={renaming}
                maxLength={24}
                aria-label={t('bots:new.name')}
                disabled={saving}
                onChange={(event) => setRenaming(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Escape') setRenaming(null)
                }}
              />
              <Button type="submit" size="sm" disabled={saving || !renaming.trim()}>
                {t('bots:details.save')}
              </Button>
              <Button type="button" size="sm" variant="ghost" onClick={() => setRenaming(null)}>
                {t('common:ui.cancel')}
              </Button>
            </form>
          ) : (
            <div className="flex min-w-0 items-center gap-1">
              <h2 className="m-0 truncate text-lg font-semibold text-content">{bot.name}</h2>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={t('bots:settings.rename')}
                title={t('bots:settings.rename')}
                onClick={() => setRenaming(bot.name)}
              >
                <Pencil />
              </Button>
            </div>
          )}
          <p className="m-0 mt-0.5 flex min-w-0 items-center gap-1.5 text-xs text-content-muted">
            <span
              aria-hidden="true"
              className={cn(
                'size-1.5 flex-none rounded-full',
                bot.enabled ? 'bg-success' : 'bg-content-faint',
              )}
            />
            <span className="truncate">{status}</span>
          </p>
        </div>
        {/* 关掉 = 暂停：聊天和定时都不再叫醒它，和它的对话照常能用。 */}
        <label
          className="flex flex-none items-center gap-2 text-[13px] text-content-secondary"
          title={t('bots:details.acceptsWork')}
        >
          {t('bots:settings.accepting')}
          <Switch
            checked={bot.enabled}
            disabled={saving}
            aria-label={t('bots:details.acceptsWork')}
            onCheckedChange={(enabled) => void onSave({ enabled })}
          />
        </label>
      </div>
      {choosingLook && (
        <BotAvatarPicker
          value={bot.avatar}
          disabled={saving}
          onChange={(avatar) => void onSave({ avatar })}
        />
      )}
    </div>
  )
}

type Counts = { memory: number | null; skills: number | null; schedules: number | null }

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
  const [open, setOpen] = useState<ReadonlySet<string>>(
    () => new Set(openSection ? [openSection] : []),
  )
  const [counts, setCounts] = useState<Counts>({ memory: null, skills: null, schedules: null })
  useEffect(() => setInstructions(bot.instructions), [bot.id, bot.instructions])
  // 行里的摘要要写几条记忆、几个技能、几个定时：打开页面时先数一遍，读不到就不写数字；
  // 展开后由各自的列表接着报。
  useEffect(() => {
    const controller = new AbortController()
    const count = (key: keyof Counts, load: Promise<unknown[]>) =>
      load
        .then((items) => setCounts((current) => ({ ...current, [key]: items.length })))
        .catch(() => {})
    void count('memory', fetchBotMemory(bot.id, controller.signal))
    void count('skills', fetchBotSkills(bot.id, controller.signal))
    void count('schedules', fetchBotSchedules(bot.id, controller.signal))
    return () => controller.abort()
  }, [bot.id])
  // 从别处跳到某个分区时（招呼里的聊天接入）：它可能在屏幕下面，滚到能看见。
  const rootRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (openSection)
      rootRef.current
        ?.querySelector(`[data-bot-setting="${CSS.escape(openSection)}"]`)
        ?.scrollIntoView()
  }, [openSection])

  const save = async (patch: Parameters<typeof updateBot>[1]) => {
    setSaving(true)
    try {
      upsert(await updateBot(bot.id, patch))
      return true
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
      return false
    } finally {
      setSaving(false)
    }
  }
  const toggle = (id: string) =>
    setOpen((current) => {
      const next = new Set(current)
      if (!next.delete(id)) next.add(id)
      return next
    })
  const row = (id: string) => ({ id, open: open.has(id), onToggle: () => toggle(id) })
  // 还没数出来时不写；定时一个没有时写“未设置”，记忆和技能写“0 条”“0 个”。
  const memorySummary =
    counts.memory === null ? '' : t('bots:memory.count', { count: counts.memory })
  const skillsSummary =
    counts.skills === null ? '' : t('bots:skills.count', { count: counts.skills })
  const schedulesSummary =
    counts.schedules === null
      ? ''
      : counts.schedules
        ? t('bots:settings.schedulesCount', { count: counts.schedules })
        : t('bots:settings.notSet')
  // 列表报数的回调要稳定：定时列表按它重新加载，每次渲染换一个会反复去读。
  const setCount = useMemo(() => {
    const make = (key: keyof Counts) => (value: number) =>
      setCounts((current) => (current[key] === value ? current : { ...current, [key]: value }))
    return { memory: make('memory'), skills: make('skills'), schedules: make('schedules') }
  }, [])

  const environment = environments.find((item) => item.id === bot.workspace.environmentId)
  const openCommands = bot.commands.filter((command) => command.audience === 'everyone').length
  return (
    <div ref={rootRef} className="flex flex-col gap-8 pb-4">
      <BotHero bot={bot} environment={environment} saving={saving} onSave={save} />
      <section aria-labelledby="bot-instructions-title" className="flex flex-col gap-1.5">
        <h2
          id="bot-instructions-title"
          className="m-0 text-[15px] leading-6 font-medium text-content"
        >
          {t('bots:details.instructions')}
        </h2>
        <Textarea
          value={instructions}
          rows={3}
          maxLength={12_000}
          aria-labelledby="bot-instructions-title"
          placeholder={t('bots:details.instructionsPlaceholder')}
          onChange={(event) => setInstructions(event.target.value)}
        />
        {instructions !== bot.instructions && (
          <div className="flex justify-end">
            <Button size="sm" disabled={saving} onClick={() => void save({ instructions })}>
              {t('bots:details.save')}
            </Button>
          </div>
        )}
      </section>

      <SettingsSection title={t('bots:settings.groupWork')}>
        <ManagedRow
          {...row('computer')}
          title={t('bots:settings.computer')}
          summary={t('bots:settings.computerSummary', {
            computer: environmentLabel(t, environment),
            folder: botFolderName(bot.workspace.cwd),
          })}
        >
          <BotComputerTab
            bot={bot}
            environments={environments}
            onEnvironmentsChanged={onEnvironmentsChanged}
            onError={onError}
          />
        </ManagedRow>
        <BotModelSection bot={bot} saving={saving} onSave={(patch) => void save(patch)} />
        <SettingsRow
          title={t('bots:details.permission')}
          description={t('bots:details.permissionHint')}
          control={
            <AppSelect
              value={bot.executionMode}
              className="w-48"
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
          }
        />
        <SettingsRow
          title={t('bots:details.tools')}
          description={t('bots:details.toolsHint')}
          control={
            <AppSelect
              value={botToolsPreset(bot.tools)}
              className="w-48"
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
          }
        />
        <SettingsRow
          title={t('bots:details.parallel')}
          description={t('bots:details.parallelHint')}
          control={
            <AppSelect
              value={String(bot.maxParallelAgents)}
              className="w-48"
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
          }
        />
      </SettingsSection>

      <SettingsSection title={t('bots:settings.groupReach')}>
        <ManagedRow
          {...row('chats')}
          title={t('bots:settings.chats')}
          summary={
            bot.channels.length
              ? t('bots:settings.chatsCount', { count: bot.channels.length })
              : t('bots:settings.notSet')
          }
        >
          <ChannelEditor bot={bot} onError={onError} />
        </ManagedRow>
        <ManagedRow
          {...row('commands')}
          title={t('bots:commands.title')}
          summary={
            bot.commands.length
              ? t('bots:commands.count', { count: bot.commands.length, open: openCommands })
              : t('bots:settings.notSet')
          }
        >
          <BotCommandsSection bot={bot} onError={onError} />
        </ManagedRow>
      </SettingsSection>

      <SettingsSection title={t('bots:settings.groupOwn')}>
        <BotProactiveSection
          bot={bot}
          saving={saving}
          onSave={(proactive) => void save({ proactive })}
        />
        <ManagedRow
          {...row('schedules')}
          title={t('bots:settings.schedules')}
          summary={schedulesSummary}
        >
          <ScheduleEditor bot={bot} onError={onError} onCount={setCount.schedules} />
        </ManagedRow>
      </SettingsSection>

      <SettingsSection title={t('bots:settings.groupMemory')}>
        <ManagedRow {...row('memory')} title={t('bots:memory.title')} summary={memorySummary}>
          <BotMemorySection bot={bot} onError={onError} onCount={setCount.memory} />
        </ManagedRow>
        <SettingsRow
          title={t('bots:memory.readGlobal')}
          description={t('bots:memory.readGlobalHint')}
          control={
            <Switch
              checked={bot.readGlobalMemory}
              disabled={saving}
              aria-label={t('bots:memory.readGlobal')}
              onCheckedChange={(readGlobalMemory) => void save({ readGlobalMemory })}
            />
          }
        />
        <ManagedRow {...row('skills')} title={t('bots:skills.title')} summary={skillsSummary}>
          <BotSkillsSection bot={bot} onError={onError} onCount={setCount.skills} />
        </ManagedRow>
      </SettingsSection>
    </div>
  )
}
