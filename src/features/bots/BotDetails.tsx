// Bot 的详情栏（设计稿 botim-chat / botim-changes / botim-computer）：三个标签。
// 详情：头像、它记得的工作说明（可编辑）、权限上限、交付方式、暂停。
// 改动：待确认任务的文件列表与 diff。电脑：它在哪台电脑、哪个目录工作；第一版只有本机。
import { lazy, Suspense, useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { InlineEmptyState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { FileDiff, Monitor } from 'lucide-react'
import { BotAvatar } from './BotAvatar'
import { fetchBotTaskDiff, updateBot, type Bot, type BotDiff, type BotTask } from './bots-api'
import { deliveryLabel, executionModeLabel } from './bot-labels'
import { botAvatarState } from './bot-presentation'
import { useBotsStore } from './bots-store'

export type BotDetailsTab = 'details' | 'changes' | 'computer'

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
  onError,
}: {
  bot: Bot
  reviewTask: BotTask | null
  workingTask: BotTask | null
  tab: BotDetailsTab
  onTabChange: (tab: BotDetailsTab) => void
  onError: (message: string) => void
}) {
  const { t } = useI18n()
  const upsert = useBotsStore((state) => state.upsert)
  const [instructions, setInstructions] = useState(bot.instructions)
  const [saving, setSaving] = useState(false)
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

  return (
    <Tabs
      value={tab}
      onValueChange={(value) => onTabChange(value as BotDetailsTab)}
      className="flex min-h-0 flex-1 flex-col"
    >
      <TabsList className="mx-3 mt-3">
        <TabsTrigger value="details">{t('bots:details.tabDetails')}</TabsTrigger>
        <TabsTrigger value="changes">
          {reviewTask
            ? t('bots:details.tabChangesCount', { count: reviewTask.result?.changedFiles ?? 0 })
            : t('bots:details.tabChanges')}
        </TabsTrigger>
        <TabsTrigger value="computer">{t('bots:details.tabComputer')}</TabsTrigger>
      </TabsList>
      <TabsContent value="details" className="min-h-0 flex-1 overflow-y-auto px-4 pb-4">
        <div className="flex flex-col items-center gap-1 border-b border-stroke py-4">
          <BotAvatar avatar={bot.avatar} state={botAvatarState(bot)} size={72} />
          <div className="text-base font-semibold text-content">{bot.name}</div>
          <div className="text-xs text-content-muted">@{bot.name}</div>
        </div>
        <section className="border-b border-stroke py-3.5">
          <h3 className="mb-1 text-xs font-medium text-content-muted">
            {t('bots:details.instructions')}
          </h3>
          <p className="mb-2 text-xs text-content-muted">{t('bots:details.instructionsHint')}</p>
          <Textarea
            value={instructions}
            rows={5}
            maxLength={12_000}
            aria-label={t('bots:details.instructions')}
            placeholder={t('bots:details.instructionsPlaceholder')}
            onChange={(event) => setInstructions(event.target.value)}
          />
          <div className="mt-2 flex justify-end">
            <Button
              size="sm"
              variant="outline"
              disabled={saving || instructions === bot.instructions}
              onClick={() => void save({ instructions })}
            >
              {t('bots:details.save')}
            </Button>
          </div>
        </section>
        <section className="flex flex-col gap-3 py-3.5">
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
            <span className="flex-none">{t('bots:details.delivery')}</span>
            <AppSelect
              value={bot.delivery}
              className="w-44"
              aria-label={t('bots:details.delivery')}
              disabled={saving}
              onChange={(event) => void save({ delivery: event.target.value as Bot['delivery'] })}
            >
              {(['diff', 'commit', 'reply-only'] as const).map((delivery) => (
                <option key={delivery} value={delivery}>
                  {deliveryLabel(t, delivery)}
                </option>
              ))}
            </AppSelect>
          </label>
        </section>
      </TabsContent>
      <TabsContent value="changes" className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        <ChangesTab task={reviewTask} />
      </TabsContent>
      <TabsContent value="computer" className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        <h3 className="text-sm font-semibold text-content">
          {t('bots:computer.title', { name: bot.name })}
        </h3>
        <p className="mt-1 text-xs text-content-muted">{t('bots:computer.description')}</p>
        <div className="mt-3 flex flex-col gap-2 text-sm">
          <AppSelect value="local" aria-label={t('bots:computer.runsOn')} disabled>
            <option value="local">{t('bots:computer.local')}</option>
          </AppSelect>
          <p className="text-xs text-content-muted">{t('bots:computer.moreSoon')}</p>
          <div className="mt-2 flex items-center gap-2 font-mono text-xs text-content-secondary">
            <Monitor className="size-3.5" aria-hidden="true" />
            <span className="truncate">{bot.workspace.cwd}</span>
          </div>
          <SegmentedControl
            value={bot.workspace.isolation}
            disabled={saving}
            onValueChange={(isolation) => void save({ workspace: { ...bot.workspace, isolation } })}
            options={[
              { value: 'worktree', label: t('bots:computer.isolated') },
              { value: 'shared', label: t('bots:computer.shared') },
            ]}
          />
          {workingTask?.workspace?.branch && (
            <p className="text-xs text-content-muted">
              {t('bots:computer.currentBranch', { branch: workingTask.workspace.branch })}
            </p>
          )}
        </div>
      </TabsContent>
    </Tabs>
  )
}
