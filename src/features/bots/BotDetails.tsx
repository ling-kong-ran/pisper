// Bot 的侧栏（按“第一眼只看到对话”简化）：平时收起，点右上角设置或卡片上的“查看改动”才打开。
// 设置：头像、名字和“它负责什么”在最上面；电脑、在聊天里叫它、定时、高级（权限、交付、工具、暂停）
// 都收在折叠分区里，用默认值就能用，想改时再展开。
// 改动：有待确认的改动时才出现这个标签，列文件与 diff。
import { lazy, Suspense, useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { InlineEmptyState } from '@/components/app/PageState'
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@/components/ui/accordion'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { FileDiff } from 'lucide-react'
import { BotAvatar } from './BotAvatar'
import { BotAvatarPicker } from './BotAvatarPicker'
import { BotComputerTab } from './BotComputerTab'
import { ChannelEditor, ScheduleEditor } from './BotTriggers'
import {
  fetchBotTaskDiff,
  updateBot,
  type Bot,
  type BotDiff,
  type BotEnvironment,
  type BotTask,
} from './bots-api'
import { environmentLabel } from './bot-environments'
import { executionModeLabel } from './bot-labels'
import { BOT_READ_ONLY_TOOLS, botAvatarState, botToolsPreset } from './bot-presentation'
import { useBotsStore } from './bots-store'

export type BotDetailsTab = 'settings' | 'changes'

// 改动用会话里同一套 diff 查看器（按需懒加载）。
const InlineGitDiff = lazy(() =>
  import('@/features/chat/public-diff').then((module) => ({ default: module.InlineGitDiff })),
)

function ChangesTab({ task }: { task: BotTask | null }) {
  const { t } = useI18n()
  const [diff, setDiff] = useState<BotDiff | null>(null)
  const [error, setError] = useState('')
  const taskId = task?.id ?? ''
  const version = task?.updatedAt ?? ''
  useEffect(() => {
    setDiff(null)
    setError('')
    if (!taskId) return
    const controller = new AbortController()
    fetchBotTaskDiff(taskId, controller.signal)
      .then(setDiff)
      .catch((caught: unknown) => {
        if (!controller.signal.aborted)
          setError(caught instanceof Error ? caught.message : String(caught))
      })
    return () => controller.abort()
  }, [taskId, version])
  if (!task) return <InlineEmptyState compact icon={FileDiff} title={t('bots:details.noChanges')} />
  if (error)
    return (
      <p role="alert" className="text-sm text-content-secondary">
        {error}
      </p>
    )
  if (!diff) return <p className="text-sm text-content-muted">{t('bots:details.loadingChanges')}</p>
  return (
    <>
      {/* 先列改了哪些文件，再看逐行 diff。 */}
      <ul role="list" className="m-0 mb-3 flex list-none flex-col gap-1 p-0">
        {diff.files.map((file) => (
          <li key={file.path} className="flex min-w-0 items-center gap-2 font-mono text-xs">
            <span className="w-4 flex-none text-content-muted">{file.status}</span>
            <span className="truncate text-content-secondary">{file.path}</span>
          </li>
        ))}
      </ul>
      <Suspense
        fallback={<p className="text-sm text-content-muted">{t('bots:details.loadingChanges')}</p>}
      >
        <InlineGitDiff diff={diff.diff} truncated={diff.truncated} mode="unified" fill />
      </Suspense>
    </>
  )
}

export function BotDetails({
  bot,
  reviewTask,
  workingTask,
  tab,
  onTabChange,
  environments,
  onEnvironmentsChanged,
  onError,
}: {
  bot: Bot
  reviewTask: BotTask | null
  workingTask: BotTask | null
  tab: BotDetailsTab
  onTabChange: (tab: BotDetailsTab) => void
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
  const settings = (
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
              workingTask={workingTask}
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

  // 没有待确认的改动时只有设置，不放只有一个标签的标签栏。
  if (!reviewTask) return <div className="min-h-0 flex-1 overflow-y-auto px-4 pt-4">{settings}</div>
  return (
    <Tabs
      value={tab}
      onValueChange={(value) => onTabChange(value as BotDetailsTab)}
      className="flex min-h-0 flex-1 flex-col"
    >
      <TabsList className="mx-3 mt-3">
        <TabsTrigger value="changes">
          {t('bots:details.tabChangesCount', { count: reviewTask.result?.changedFiles ?? 0 })}
        </TabsTrigger>
        <TabsTrigger value="settings">{t('bots:settings.title')}</TabsTrigger>
      </TabsList>
      <TabsContent value="changes" className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        <ChangesTab task={reviewTask} />
      </TabsContent>
      <TabsContent value="settings" className="min-h-0 flex-1 overflow-y-auto px-4 pt-3">
        {settings}
      </TabsContent>
    </Tabs>
  )
}
