// Bot 的展示逻辑（纯函数）：头像状态、侧栏那一句动态。文案键是字面量，i18n 检查逐个核对。
import type { Translate } from '@/app/navigation'
import type { BotAvatarState } from './avatar'
import type { Bot, BotTaskStatus } from './bots-api'

const ACTIVE: ReadonlySet<BotTaskStatus> = new Set(['queued', 'preparing', 'running'])

// “只读”预设：能读、能搜代码，不能改文件、不能跑命令。Runtime 在执行前按这份名单拦截。
export const BOT_READ_ONLY_TOOLS = ['read', 'grep', 'find', 'ls'] as const

export type BotToolsPreset = 'all' | 'read-only' | 'custom'

export function botToolsPreset(tools: readonly string[] | null): BotToolsPreset {
  if (tools === null) return 'all'
  const sorted = [...tools].sort()
  return sorted.length === BOT_READ_ONLY_TOOLS.length &&
    [...BOT_READ_ONLY_TOOLS].sort().every((name, index) => sorted[index] === name)
    ? 'read-only'
    : 'custom'
}

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
  if (activity.waiting === 'environment') return t('bots:activity.environment')
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
