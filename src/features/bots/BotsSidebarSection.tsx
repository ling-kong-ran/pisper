// 侧栏里的 Bots 分组（设计稿 botim-chat）：每个 Bot 一行，像联系人——头像随状态动、名字、最近一句；
// 需要你时那句话变琥珀色。只在实验功能 Bots 打开时出现。点“+”新建。
import './bots-messages'
import { useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { Plus } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useBetaFeature } from '@/features/labs/public-flags'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import { botActivityLine, botAvatarState } from './bot-presentation'
import { useBotsPolling, useBotsStore } from './bots-store'
import { NewBotDialog } from './NewBotDialog'

export function BotsSidebarSection() {
  const { t } = useI18n()
  const enabled = useBetaFeature('bots')
  const bots = useBotsStore((state) => state.bots)
  const navigate = useNavigate()
  const location = useLocation()
  const [creating, setCreating] = useState(false)
  useBotsPolling(enabled)
  if (!enabled) return null
  const current = /^\/bots\/([^/]+)/.exec(location.pathname)?.[1]
  return (
    <section data-sidebar-bots aria-label={t('bots:sidebar.title')} className="mb-2">
      <div className="flex h-[26px] items-center gap-1.5 px-2.5 text-xs font-medium text-content-muted">
        <h2 className="flex-1">{t('bots:sidebar.title')}</h2>
        <button
          type="button"
          className="grid size-6 place-items-center rounded-md hover:bg-surface-hover hover:text-content"
          aria-label={t('bots:sidebar.newBot')}
          title={t('bots:sidebar.newBot')}
          onClick={() => setCreating(true)}
        >
          <Plus className="size-3.5" aria-hidden="true" />
        </button>
      </div>
      <ul role="list" className="m-0 list-none p-0">
        {bots.map((bot) => {
          const activity = bot.activity
          const line = botActivityLine(t, bot)
          return (
            <li key={bot.id}>
              <button
                type="button"
                data-sidebar-bot={bot.id}
                aria-current={current === bot.id ? 'page' : undefined}
                className={cn(
                  'flex h-[46px] w-full min-w-0 items-center gap-2.5 rounded-[9px] px-2.5 text-left hover:bg-surface-hover pointer-coarse:h-14',
                  current === bot.id && 'bg-surface-active hover:bg-surface-active',
                )}
                onClick={() => navigate(`/bots/${encodeURIComponent(bot.id)}`)}
              >
                <BotAvatar avatar={bot.avatar} state={botAvatarState(bot)} size={30} />
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-[13.5px] font-medium text-content">
                    {bot.name}
                  </span>
                  <span
                    className={cn(
                      'truncate text-xs text-content-muted',
                      activity?.needsYou && 'text-st-attn',
                    )}
                  >
                    {line}
                  </span>
                </span>
              </button>
            </li>
          )
        })}
      </ul>
      <NewBotDialog
        open={creating}
        onOpenChange={setCreating}
        onCreated={(bot) => navigate(`/bots/${encodeURIComponent(bot.id)}`)}
      />
    </section>
  )
}
