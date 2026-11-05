// Bot 的设置页（路由 /bots/:botId）：Bot 就是一个长期会话加上它的设定，和它说话在它的会话里
// （侧栏点 Bot 直接打开会话）；这里只放设定——头像、名字、它负责什么、电脑、聊天、定时、高级。
// 顶栏左边“和它聊天”回到它的会话。/bots 没有 Bot 时给新建入口，有 Bot 时打开第一个的设置。
import './bots-messages'
import { useCallback, useState } from 'react'
import { Navigate, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { ChevronLeft, Plus } from 'lucide-react'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { useBetaFeature } from '@/features/labs/public-flags'
import { BotDetails } from './BotDetails'
import { useBotsPolling, useBotsStore } from './bots-store'
import { NewBotDialog } from './NewBotDialog'
import { useBotEnvironments } from './use-bot-environments'
import { useOpenBot } from './use-open-bot'

export function BotsPage({ notify }: { notify: Notify }) {
  const { t } = useI18n()
  const enabled = useBetaFeature('bots')
  const { botId = '' } = useParams()
  const navigate = useNavigate()
  const bots = useBotsStore((state) => state.bots)
  const loaded = useBotsStore((state) => state.loaded)
  const [creating, setCreating] = useState(false)
  const reportError = useCallback((message: string) => notify(message, 'error'), [notify])
  const openBot = useOpenBot(reportError)
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
          onCreated={(created) => void openBot(created)}
        />
      </>
    )
  return (
    <BotSettingsPage
      key={bot.id}
      botId={bot.id}
      onOpenChat={() => void openBot(bot)}
      onError={reportError}
    />
  )
}

function BotSettingsPage({
  botId,
  onOpenChat,
  onError,
}: {
  botId: string
  onOpenChat: () => void
  onError: (message: string) => void
}) {
  const { t } = useI18n()
  const bot = useBotsStore((state) => state.bots.find((item) => item.id === botId))
  const loadBots = useBotsStore((state) => state.load)
  const environments = useBotEnvironments()
  // 招呼里的“在聊天软件里找我”直接打开聊天分区；其余情况分区都收着。
  const [searchParams] = useSearchParams()
  const openSection = searchParams.get('section') === 'chats' ? 'chats' : undefined
  if (!bot) return null
  return (
    <div data-bot-settings={bot.id} className="flex h-full min-h-0 w-full min-w-0 flex-col">
      {/* 与会话顶栏同高 52，空白处是窗口拖拽区；Windows 自绘标题栏时右上 144px 让给窗口按钮。 */}
      <header
        data-window-drag-region
        className="flex h-[52px] flex-none items-center gap-2 border-b border-stroke px-3 [[data-custom-titlebar]_&]:pr-[144px]"
      >
        <Button variant="ghost" size="sm" onClick={onOpenChat}>
          <ChevronLeft aria-hidden="true" />
          {t('bots:settingsPage.backToChat', { name: bot.name })}
        </Button>
        <h1 className="sr-only">{t('bots:settingsPage.title', { name: bot.name })}</h1>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-[min(640px,calc(100%-32px))] py-6">
          <BotDetails
            bot={bot}
            environments={environments.environments}
            openSection={openSection}
            onEnvironmentsChanged={() => {
              void environments.refresh()
              void loadBots()
            }}
            onError={onError}
          />
        </div>
      </div>
    </div>
  )
}
