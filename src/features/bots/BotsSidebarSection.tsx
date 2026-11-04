// 侧栏里的 Bots 分组（设计稿 botim-chat）：每个 Bot 一行，像联系人——头像随状态动、名字、最近一句；
// 需要你时那句话变琥珀色。只在实验功能 Bots 打开时出现。点“+”新建。
// Bot 多时只显示最要紧的 3 个（需要你 → 干活中 → 最近活动），其余点“显示全部”就地展开（sidebarBots）。
import './bots-messages'
import { useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { Plus } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useBetaFeature } from '@/features/labs/public-flags'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import { botActivityLine, botAvatarState, SIDEBAR_BOT_LIMIT, sidebarBots } from './bot-presentation'
import { useBotsPolling, useBotsStore } from './bots-store'
import { NewBotDialog } from './NewBotDialog'

export function BotsSidebarSection() {
  const { t } = useI18n()
  const enabled = useBetaFeature('bots')
  const bots = useBotsStore((state) => state.bots)
  const navigate = useNavigate()
  const location = useLocation()
  const [creating, setCreating] = useState(false)
  const [expanded, setExpanded] = useState(false)
  useBotsPolling(enabled)
  if (!enabled) return null
  const current = /^\/bots\/([^/]+)/.exec(location.pathname)?.[1]
  const { shown, hidden } = sidebarBots(bots, { expanded, currentId: current })
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
        {shown.map((bot) => {
          const activity = bot.activity
          const line = botActivityLine(t, bot)
          return (
            <li key={bot.id}>
              <button
                type="button"
                data-sidebar-bot={bot.id}
                aria-current={current === bot.id ? 'page' : undefined}
                // 一行：头像（随状态动）+ 名字；最近一句放进悬停提示与读屏，需要你时名字后加琥珀点。
                title={line}
                className={cn(
                  'flex h-8 w-full min-w-0 items-center gap-2.5 rounded-[var(--r-md)] px-2.5 text-left hover:bg-surface-hover pointer-coarse:h-11',
                  current === bot.id && 'bg-surface-active hover:bg-surface-active',
                )}
                onClick={() => navigate(`/bots/${encodeURIComponent(bot.id)}`)}
              >
                <BotAvatar avatar={bot.avatar} state={botAvatarState(bot)} size={20} />
                <span className="min-w-0 flex-1 truncate text-[13.5px] text-content">
                  {bot.name}
                  <span className="sr-only">{` · ${line}`}</span>
                </span>
                {activity?.needsYou && (
                  <span aria-hidden="true" className="size-1.5 flex-none rounded-full bg-st-attn" />
                )}
              </button>
            </li>
          )
        })}
      </ul>
      {(hidden > 0 || (expanded && bots.length > SIDEBAR_BOT_LIMIT)) && (
        <button
          type="button"
          data-sidebar-bots-more
          aria-expanded={expanded}
          className="flex h-7 w-full items-center rounded-[var(--r-md)] px-2.5 pl-9 text-left text-xs text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11"
          onClick={() => setExpanded(!expanded)}
        >
          {expanded
            ? t('bots:sidebar.showLess')
            : t('bots:sidebar.showAll', { count: bots.length })}
        </button>
      )}
      <NewBotDialog
        open={creating}
        onOpenChange={setCreating}
        onCreated={(bot) => navigate(`/bots/${encodeURIComponent(bot.id)}`)}
      />
    </section>
  )
}
