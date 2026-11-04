// Bot 页（设计稿 botim-chat）：和一个 Bot 的长期对话 + 右侧详情栏。路由 /bots/:botId；/bots 跳到第一个 Bot。
// 头部：头像、名字、“它的电脑”、详情栏开关。和 Grok Bot 一样，每个 Bot 的对话就在主窗口里，点侧栏切换，不另开窗口。
// 输入框回车发送、Shift+回车换行，有改动等你确认时说的话会在同一个分支上接着改。
import './bots-messages'
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import { Navigate, useNavigate, useParams } from 'react-router-dom'
import { ArrowUp, Plus, Settings2 } from 'lucide-react'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { useBetaFeature } from '@/features/labs/public-flags'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import { environmentLabel } from './bot-environments'
import { BotConversation } from './BotConversation'
import { BotDetails, type BotDetailsTab } from './BotDetails'
import { sendBotMessage, type BotTask } from './bots-api'
import { botActivityLine, botAvatarState, isActiveTaskStatus } from './bot-presentation'
import { useBotsPolling, useBotsStore } from './bots-store'
import { NewBotDialog } from './NewBotDialog'
import { useBotEnvironments } from './use-bot-environments'
import { useBotTasks } from './use-bot-tasks'

export function BotsPage({ notify }: { notify: Notify }) {
  const { t } = useI18n()
  const enabled = useBetaFeature('bots')
  const { botId = '' } = useParams()
  const navigate = useNavigate()
  const bots = useBotsStore((state) => state.bots)
  const loaded = useBotsStore((state) => state.loaded)
  const [creating, setCreating] = useState(false)
  useBotsPolling(enabled)
  const bot = bots.find((item) => item.id === botId) ?? null

  if (!enabled)
    return (
      <PageState
        kind="empty"
        title={t('bots:page.disabledTitle')}
        description={t('bots:page.disabledDescription')}
        action={<Button onClick={() => navigate('/config/labs')}>{t('bots:page.openLabs')}</Button>}
      />
    )
  if (!loaded) return <PageState kind="loading" title={t('bots:page.loading')} />
  if (!botId && bots[0]) return <Navigate to={`/bots/${encodeURIComponent(bots[0].id)}`} replace />
  if (!bot)
    return (
      <>
        <PageState
          kind="empty"
          title={bots.length ? t('bots:page.notFound') : t('bots:page.emptyTitle')}
          description={t('bots:page.emptyDescription')}
          action={
            <Button onClick={() => setCreating(true)}>
              <Plus aria-hidden="true" />
              {t('bots:sidebar.newBot')}
            </Button>
          }
        />
        <NewBotDialog
          open={creating}
          onOpenChange={setCreating}
          onCreated={(created) => navigate(`/bots/${encodeURIComponent(created.id)}`)}
        />
      </>
    )
  return <BotChat key={bot.id} botId={bot.id} notify={notify} />
}

function BotChat({ botId, notify }: { botId: string; notify: Notify }) {
  const { t } = useI18n()
  const bot = useBotsStore((state) => state.bots.find((item) => item.id === botId))
  const loadBots = useBotsStore((state) => state.load)
  const { tasks, loaded, refresh } = useBotTasks(botId)
  const environments = useBotEnvironments()
  const [draft, setDraft] = useState('')
  const [sending, setSending] = useState(false)
  // 第一眼只看对话：侧栏平时收起，点设置或“查看改动”才打开。
  const [detailsOpen, setDetailsOpen] = useState(false)
  const [tab, setTab] = useState<BotDetailsTab>('settings')
  const scroller = useRef<HTMLDivElement>(null)
  // 任务按创建时间排列，取最近的一个。
  const reviewTask = useMemo(
    () => [...tasks].reverse().find((task) => task.status === 'review') ?? null,
    [tasks],
  )
  const workingTask = useMemo(
    () => [...tasks].reverse().find((task) => isActiveTaskStatus(task.status)) ?? null,
    [tasks],
  )

  // 新内容进来时滚到底，和会话页一样从下往上读。
  const latestUpdate = `${tasks.length}:${tasks.at(-1)?.updatedAt ?? ''}`
  useEffect(() => {
    scroller.current?.scrollTo({ top: scroller.current.scrollHeight })
  }, [latestUpdate])

  if (!bot) return null
  const changed = () => {
    void refresh()
    void loadBots()
  }
  const reportError = (message: string) => notify(message, 'error')
  const send = async () => {
    const text = draft.trim()
    if (!text || sending) return
    setSending(true)
    try {
      await sendBotMessage(bot.id, text)
      setDraft('')
      changed()
    } catch (caught) {
      reportError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSending(false)
    }
  }
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault()
      void send()
    }
  }
  const current = environments.environments.find(
    (environment) => environment.id === bot.workspace.environmentId,
  )
  const remote = current?.kind === 'node' ? current : undefined
  const showChanges = (task: BotTask) => {
    if (task.status !== 'review') return
    setTab('changes')
    setDetailsOpen(true)
  }

  return (
    <div data-bot-page={bot.id} className="flex h-full min-h-0 w-full min-w-0">
      <section className="flex min-h-0 min-w-0 flex-1 flex-col" aria-label={bot.name}>
        {/* 与会话顶栏同高 52，空白处是窗口拖拽区；Windows 自绘标题栏时窗口按钮占右上 144px：
            详情栏关着时顶栏贴着窗口右边，要让出这段，免得设置键被压在窗口按钮底下。 */}
        <header
          data-window-drag-region
          className={cn(
            'flex h-[52px] flex-none items-center gap-2.5 border-b border-stroke px-4',
            !detailsOpen && '[[data-custom-titlebar]_&]:pr-[144px]',
          )}
        >
          <BotAvatar avatar={bot.avatar} state={botAvatarState(bot)} size={32} />
          <div className="min-w-0">
            <h1 className="truncate text-[15px] font-semibold text-content">{bot.name}</h1>
            <p className="truncate text-xs text-content-muted">
              {botActivityLine(t, bot)}
              {/* 只有不在本机时才写在哪台电脑上：大多数 Bot 就在本机，不必每次都看到。 */}
              {remote &&
                ` · ${t('bots:header.onComputer', { computer: environmentLabel(t, remote) })}`}
            </p>
          </div>
          <span className="flex-1" />
          <Button
            variant="ghost"
            size="icon"
            aria-label={t('bots:settings.botTitle', { name: bot.name })}
            title={t('bots:settings.botTitle', { name: bot.name })}
            aria-pressed={detailsOpen && (tab === 'settings' || !reviewTask)}
            onClick={() => {
              if (detailsOpen && (tab === 'settings' || !reviewTask)) setDetailsOpen(false)
              else {
                setTab('settings')
                setDetailsOpen(true)
              }
            }}
          >
            <Settings2 aria-hidden="true" />
          </Button>
        </header>
        <div ref={scroller} className="min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto w-[min(700px,calc(100%-48px))] py-6">
            {loaded ? (
              <BotConversation
                bot={bot}
                tasks={tasks}
                onChanged={changed}
                onShowChanges={showChanges}
                onError={reportError}
              />
            ) : (
              <PageState kind="loading" title={t('bots:page.loading')} />
            )}
          </div>
        </div>
        <div className="mx-auto w-[min(700px,calc(100%-48px))] flex-none pb-4">
          <div className="rounded-[22px] border border-stroke-strong bg-surface-raised px-4 pt-3 pb-2 shadow-card">
            <Textarea
              value={draft}
              rows={2}
              aria-label={t('bots:composer.label', { name: bot.name })}
              placeholder={
                reviewTask
                  ? t('bots:composer.followUp', { name: bot.name })
                  : workingTask
                    ? t('bots:composer.busy', { name: bot.name })
                    : t('bots:composer.placeholder', { name: bot.name })
              }
              className="min-h-12 resize-none border-0 bg-transparent px-0 shadow-none focus-visible:ring-0"
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={onKeyDown}
            />
            <div className="flex items-center justify-end">
              <Button
                size="icon"
                className="rounded-full"
                aria-label={t('bots:composer.send')}
                disabled={sending || !draft.trim() || !bot.enabled}
                onClick={() => void send()}
              >
                <ArrowUp aria-hidden="true" />
              </Button>
            </div>
          </div>
          {!bot.enabled && (
            <p className="mt-1.5 text-center text-[11.5px] text-content-faint">
              {t('bots:activity.paused')}
            </p>
          )}
        </div>
      </section>
      <aside
        aria-label={t('bots:settings.botTitle', { name: bot.name })}
        className={cn(
          // 窄屏：详情栏盖在对话上，带关闭按钮。
          'flex min-h-0 w-[380px] flex-none flex-col border-l border-stroke bg-surface max-[900px]:fixed max-[900px]:inset-0 max-[900px]:z-40 max-[900px]:w-full max-[900px]:border-l-0',
          !detailsOpen && 'hidden',
        )}
      >
        {/* Windows 自绘标题栏：详情栏贴着窗口右边，顶上留出与顶栏同高的拖动区，标签从窗口按钮下方开始。 */}
        <div
          aria-hidden="true"
          data-window-drag-region
          className="hidden h-[52px] flex-none [[data-custom-titlebar]_&]:block"
        />
        <div className="hidden justify-end px-3 pt-3 max-[900px]:flex">
          <Button variant="outline" onClick={() => setDetailsOpen(false)}>
            {t('bots:header.closeDetails')}
          </Button>
        </div>
        <BotDetails
          bot={bot}
          reviewTask={reviewTask}
          workingTask={workingTask}
          tab={tab}
          onTabChange={setTab}
          environments={environments.environments}
          onEnvironmentsChanged={() => {
            void environments.refresh()
            void loadBots()
          }}
          onError={reportError}
        />
      </aside>
    </div>
  )
}
