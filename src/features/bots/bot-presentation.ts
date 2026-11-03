// Bot 的展示逻辑（纯函数）：头像状态、侧栏那一句动态。文案键是字面量，i18n 检查逐个核对。
import type { Translate } from '@/app/navigation'
import type { BotAvatarState } from './avatar'
import type { Bot, BotTaskStatus } from './bots-api'

const ACTIVE: ReadonlySet<BotTaskStatus> = new Set(['queued', 'preparing', 'running'])

export function isActiveTaskStatus(status: BotTaskStatus) {
  return ACTIVE.has(status)
}

export function botAvatarState(bot: Pick<Bot, 'activity'>): BotAvatarState {
  if (bot.activity?.needsYou) return 'need'
  if (bot.activity && ACTIVE.has(bot.activity.status)) return 'running'
  return 'idle'
}

export function botActivityLine(t: Translate, bot: Pick<Bot, 'activity' | 'enabled'>) {
  if (!bot.enabled) return t('bots:activity.paused')
  const activity = bot.activity
  if (!activity) return t('bots:activity.idle')
  if (activity.waiting === 'approval') return t('bots:activity.approval')
  if (activity.waiting === 'question') return t('bots:activity.question')
  switch (activity.status) {
    case 'review':
      return t('bots:activity.review', { count: activity.changedFiles })
    case 'failed':
      return t('bots:activity.failed')
    case 'interrupted':
      return t('bots:activity.interrupted')
    case 'queued':
    case 'preparing':
    case 'running':
      return t('bots:activity.working', { title: activity.title })
    default:
      return activity.title
  }
}
