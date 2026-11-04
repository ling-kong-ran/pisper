// @public Bot 会话的页头部件（默认导出，供 React.lazy 加载）：会话页认出这是某个 Bot 的长期会话时，
// 标题前放它的头像（随状态动），标题后放“设置”入口（打开 /bots/:id）。数据读 Bots 的列表缓存；
// Bots 实验功能关着或 Bot 已被删除时什么都不画，会话照常是普通会话。
import './bots-messages'
import { useNavigate } from 'react-router-dom'
import { Settings2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { useBetaFeature } from '@/features/labs/public-flags'
import { BotAvatar } from './BotAvatar'
import { botAvatarState } from './bot-presentation'
import { useBotsPolling, useBotsStore } from './bots-store'

export default function BotSessionChip({
  botId,
  part,
}: {
  botId: string
  part: 'avatar' | 'settings'
}) {
  const { t } = useI18n()
  const enabled = useBetaFeature('bots')
  const navigate = useNavigate()
  const bot = useBotsStore((state) => state.bots.find((item) => item.id === botId))
  // 只让头像那一份负责轮询（页头两处共用同一份缓存，不重复开轮询）。
  useBotsPolling(enabled && part === 'avatar')
  if (!enabled || !bot) return null
  if (part === 'avatar')
    return (
      <span data-session-bot={bot.id} className="flex flex-none items-center pl-1">
        <BotAvatar avatar={bot.avatar} state={botAvatarState(bot)} size={22} />
      </span>
    )
  const label = t('bots:settingsPage.title', { name: bot.name })
  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-sm"
      className="flex-none text-content-muted hover:text-content pointer-coarse:size-11"
      data-session-bot-settings={bot.id}
      aria-label={label}
      title={label}
      onClick={() => navigate(`/bots/${encodeURIComponent(bot.id)}`)}
    >
      <Settings2 aria-hidden="true" />
    </Button>
  )
}
