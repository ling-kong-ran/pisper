// Bot 的展示逻辑（纯函数）：头像状态、侧栏那一句动态。文案键是字面量，i18n 检查逐个核对。
import type { Translate } from '@/app/navigation'
import type { BotAvatarState } from './avatar'
import type { Bot } from './bots-api'

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

// 侧栏默认显示的 Bot 数：Bot 多时只留最要紧的几个，其余“显示全部”就地展开。
export const SIDEBAR_BOT_LIMIT = 3

// 侧栏里的 Bot 顺序：需要你的在前，其次正在说话的，其余保持原顺序。
// 截断时当前打开的 Bot 总是留着（不能让正在看的那个从侧栏消失）。
export function sidebarBots<T extends Pick<Bot, 'id' | 'activity'>>(
  bots: readonly T[],
  { expanded, currentId }: { expanded: boolean; currentId?: string },
): { shown: T[]; hidden: number } {
  const rank = (bot: T) => (bot.activity?.needsYou ? 0 : bot.activity?.status === 'running' ? 1 : 2)
  const ordered = bots
    .map((bot, index) => ({ bot, index }))
    .sort((left, right) => rank(left.bot) - rank(right.bot) || left.index - right.index)
    .map((entry) => entry.bot)
  if (expanded || ordered.length <= SIDEBAR_BOT_LIMIT) return { shown: ordered, hidden: 0 }
  const shown = ordered.filter(
    (bot, index) => index < SIDEBAR_BOT_LIMIT || (currentId !== undefined && bot.id === currentId),
  )
  return { shown, hidden: ordered.length - shown.length }
}

export function botAvatarState(bot: Pick<Bot, 'activity'>): BotAvatarState {
  if (bot.activity?.needsYou) return 'need'
  if (bot.activity?.status === 'running') return 'running'
  return 'idle'
}

// 侧栏悬停提示与读屏里的那一句：暂停、等批准、等回答、在忙、空闲。
export function botActivityLine(t: Translate, bot: Pick<Bot, 'activity' | 'enabled'>) {
  if (!bot.enabled) return t('bots:activity.paused')
  const activity = bot.activity
  if (activity?.waiting === 'approval') return t('bots:activity.approval')
  if (activity?.waiting === 'question') return t('bots:activity.question')
  if (activity?.status === 'running') return t('bots:activity.busy')
  return t('bots:activity.idle')
}
