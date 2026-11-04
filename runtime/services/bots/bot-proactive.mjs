// Bot 主动（C6，默认关）：空闲时按它负责的东西做只读巡看；每天在设定时间写一份简报。
// 巡看由运行时直接派一个只给只读工具的子 agent（写操作根本不在它手上）；做完结果回到对话，对话写成一条
// 安静的建议，这一轮的决定只能记、忘，不能派活或找别的 Bot——用户说“做吧”才开活。
// 这里是纯逻辑：到没到点、巡看的交代、简报与巡看结果的提示，以及每个 Bot 的上次时间（落盘）。
import { join } from 'node:path'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'

// 巡看子 agent 只能用的工具：读文件、搜索、列目录。和 Bot 自己的工具范围取交集。
export const BOT_PROACTIVE_TOOLS = Object.freeze(['read', 'grep', 'find', 'ls'])
export const BOT_BRIEF_MARKER = '[Pisper internal bot brief]'
const BOT_ID_PATTERN = /^bot_[A-Za-z0-9_-]{6,40}$/

export function isBotBriefMessage(content) {
  return String(content || '').startsWith(BOT_BRIEF_MARKER)
}

export function botProactiveTools(bot) {
  return Array.isArray(bot.tools)
    ? BOT_PROACTIVE_TOOLS.filter((name) => bot.tools.includes(name))
    : [...BOT_PROACTIVE_TOOLS]
}

// 本地日期与时刻（运行时所在时区），简报按“今天写过没有”判断。
function localDay(now) {
  const date = new Date(now)
  const pad = (value) => String(value).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

function localMinutes(now) {
  const date = new Date(now)
  return date.getHours() * 60 + date.getMinutes()
}

// 到点了吗：巡看隔 everyHours 小时一次；简报每天一次、到了设定时间之后。
export function botProactiveDue(bot, state, now) {
  const proactive = bot.proactive || {}
  const last = Date.parse(state?.lastCheckAt || '')
  const check =
    proactive.enabled === true &&
    (!Number.isFinite(last) || now - last >= (proactive.everyHours || 6) * 60 * 60 * 1000)
  const [hours, minutes] = String(proactive.briefAt || '09:00')
    .split(':')
    .map(Number)
  const brief =
    proactive.brief === true &&
    state?.lastBriefDay !== localDay(now) &&
    localMinutes(now) >= hours * 60 + minutes
  return { check, brief, day: localDay(now) }
}

export function botProactiveTask(bot) {
  return [
    `你在替 ${bot.name} 做一次只读巡看：没有人交代具体的事，按它负责的东西看一遍现在的状况。`,
    '看什么：起始目录里的仓库状态（最近的改动、TODO、明显的问题）、依赖清单与锁文件、CI 配置、它的说明里提到要关注的目录与文件。',
    '只读：你只有读文件、搜索、列目录的工具，不改文件、不跑命令、不往外发消息。',
    '汇报：先一句话说有没有值得注意的；有的话列出发现（文件与位置），值得做的写成建议（做什么、为什么），不要自己去做。没什么发现就直说。',
  ].join('\n')
}

// 巡看做完交回对话时追加的说明：写成安静的建议，不开活。
export const BOT_PROACTIVE_NOTE =
  'This was your own read-only check; nobody asked for it. Tell the user what you found in one or two quiet sentences, as a suggestion. Do not start any work and do not message other bots: the user decides ("做吧"). If nothing is worth mentioning, say so in a few words.'

// 每日简报：昨天到现在做了什么、今天排了什么，有什么建议。隐藏消息（转录不显示），对话写出简报。
export function botBriefPrompt({ bot, jobs = [], schedules = [] }) {
  const lines = [
    BOT_BRIEF_MARKER,
    `It is time for ${bot.name}'s daily brief to the user. Write it in the user's language, short: what was done since yesterday, what is scheduled today, and any suggestions. Do not start work.`,
    `Jobs in the last 24 hours: ${
      jobs.length
        ? jobs.map((job) => `${job.title || job.taskName} (${job.status})`).join('; ')
        : 'none'
    }.`,
    `Scheduled today: ${schedules.length ? schedules.join('; ') : 'nothing'}.`,
  ]
  return lines.join('\n')
}

// 每个 Bot 上次巡看、上次简报的时间：<数据目录>/bots/<botId>/proactive.json。
export class BotProactiveStore {
  constructor({ dataDir }) {
    this.dataDir = dataDir
  }

  file(botId) {
    if (!BOT_ID_PATTERN.test(String(botId || ''))) throw new Error('Bot 不存在。')
    return join(this.dataDir, 'bots', botId, 'proactive.json')
  }

  async read(botId) {
    const state = await readJson(this.file(botId), null)
    return {
      lastCheckAt: typeof state?.lastCheckAt === 'string' ? state.lastCheckAt : null,
      lastBriefDay: typeof state?.lastBriefDay === 'string' ? state.lastBriefDay : null,
    }
  }

  async write(botId, patch) {
    const state = await this.read(botId)
    await writeJsonAtomic(this.file(botId), { version: 1, ...state, ...patch })
  }
}
