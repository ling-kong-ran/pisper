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

// 侧栏默认显示的 Bot 数：Bot 多时只留最要紧的几个，其余“显示全部”就地展开。
export const SIDEBAR_BOT_LIMIT = 3

// 侧栏里的 Bot 顺序：需要你的在前，其次正在干活的，再按最近一次活动从新到旧；没活动过的保持原顺序排在最后。
// 截断时当前打开的 Bot 总是留着（不能让正在看的那个从侧栏消失）。
export function sidebarBots<T extends Pick<Bot, 'id' | 'activity'>>(
  bots: readonly T[],
  { expanded, currentId }: { expanded: boolean; currentId?: string },
): { shown: T[]; hidden: number } {
  const rank = (bot: T) =>
    bot.activity?.needsYou ? 0 : bot.activity && ACTIVE.has(bot.activity.status) ? 1 : 2
  const at = (bot: T) => Date.parse(bot.activity?.updatedAt ?? '') || 0
  const ordered = bots
    .map((bot, index) => ({ bot, index }))
    .sort(
      (left, right) =>
        rank(left.bot) - rank(right.bot) ||
        at(right.bot) - at(left.bot) ||
        left.index - right.index,
    )
    .map((entry) => entry.bot)
  if (expanded || ordered.length <= SIDEBAR_BOT_LIMIT) return { shown: ordered, hidden: 0 }
  const shown = ordered.filter(
    (bot, index) => index < SIDEBAR_BOT_LIMIT || (currentId !== undefined && bot.id === currentId),
  )
  return { shown, hidden: ordered.length - shown.length }
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
