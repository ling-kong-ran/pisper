// Bot 页（设计稿 botim-chat）：和一个 Bot 的长期对话 + 右侧详情栏。路由 /bots/:botId；/bots 跳到第一个 Bot。
// 头部：头像、名字、“它的电脑”、在独立窗口打开（网页版）、详情栏开关。输入框回车发送、Shift+回车换行，
// 有改动等你确认时说的话会在同一个分支上接着改。
import './bots-messages'
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import { Navigate, useNavigate, useParams } from 'react-router-dom'
import { ArrowUp, ExternalLink, Monitor, PanelRight, Plus } from 'lucide-react'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { useBetaFeature } from '@/features/labs/public-flags'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import { BotConversation } from './BotConversation'
import { BotDetails, type BotDetailsTab } from './BotDetails'
import { sendBotMessage, type BotTask } from './bots-api'
import { botActivityLine, botAvatarState, isActiveTaskStatus } from './bot-presentation'
import { useBotsPolling, useBotsStore } from './bots-store'
import { NewBotDialog } from './NewBotDialog'
import { useBotTasks } from './use-bot-tasks'

// 独立窗口只在网页版里用浏览器窗口实现；Tauri 壳（桌面与手机 App）里的多窗口还没有桥接。
function canPopOut() {
  return typeof window !== 'undefined' && !window.__TAURI__ && !window.__TAURI_INTERNALS__
}

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
  const [draft, setDraft] = useState('')
  const [sending, setSending] = useState(false)
  const [detailsOpen, setDetailsOpen] = useState(() => window.innerWidth >= 1180)
  const [tab, setTab] = useState<BotDetailsTab>('details')
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
  const showChanges = (task: BotTask) => {
    if (task.status !== 'review') return
    setTab('changes')
    setDetailsOpen(true)
  }

  return (
    <div data-bot-page={bot.id} className="flex h-full min-h-0 w-full min-w-0">
      <section className="flex min-h-0 min-w-0 flex-1 flex-col" aria-label={bot.name}>
        <header className="flex h-14 flex-none items-center gap-2.5 border-b border-stroke px-4">
          <BotAvatar avatar={bot.avatar} state={botAvatarState(bot)} size={32} />
          <div className="min-w-0">
            <h1 className="truncate text-[15px] font-semibold text-content">{bot.name}</h1>
            <p className="truncate text-xs text-content-muted">{botActivityLine(t, bot)}</p>
          </div>
          <span className="flex-1" />
          <Button
            variant="outline"
            size="sm"
            aria-label={t('bots:header.computerLabel', { name: bot.name })}
            onClick={() => {
              setTab('computer')
              setDetailsOpen(true)
            }}
          >
            <Monitor aria-hidden="true" />
            <span className="text-content-muted max-[720px]:hidden">
              {t('bots:header.computer', { name: bot.name })}
            </span>
            {t('bots:computer.local')}
          </Button>
          {canPopOut() && (
            <Button
              variant="ghost"
              size="icon"
              aria-label={t('bots:header.popOut')}
              title={t('bots:header.popOut')}
              onClick={() =>
                window.open(
                  `${location.pathname}${location.search}#/bots/${encodeURIComponent(bot.id)}`,
                  `pisper-bot-${bot.id}`,
                  'popup,width=980,height=860',
                )
              }
            >
              <ExternalLink aria-hidden="true" />
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon"
            aria-label={t('bots:header.details')}
            aria-pressed={detailsOpen}
            onClick={() => setDetailsOpen((open) => !open)}
          >
            <PanelRight aria-hidden="true" />
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
          <p className="mt-1.5 text-center text-[11.5px] text-content-faint">
            {bot.enabled ? t('bots:composer.hint', { name: bot.name }) : t('bots:activity.paused')}
          </p>
        </div>
      </section>
      <aside
        aria-label={t('bots:header.details')}
        className={cn(
          // 窄屏：详情栏盖在对话上，带关闭按钮。
          'flex min-h-0 w-[380px] flex-none flex-col border-l border-stroke bg-surface max-[900px]:fixed max-[900px]:inset-0 max-[900px]:z-40 max-[900px]:w-full max-[900px]:border-l-0',
          !detailsOpen && 'hidden',
        )}
      >
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
          onError={reportError}
        />
      </aside>
    </div>
  )
}
