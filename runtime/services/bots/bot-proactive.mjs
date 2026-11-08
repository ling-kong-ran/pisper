// Bot 主动（C6，默认关）：空闲时按它负责的东西做只读巡看；每天在设定时间写一份简报。
// 巡看由运行时直接派一个只给只读工具的子 agent（写操作根本不在它手上）；做完结果回到对话，对话写成一条
// 安静的建议，这一轮的决定只能记、忘，不能派活或找别的 Bot——用户说“做吧”才开活。
// 这里是纯逻辑：到没到点、巡看的交代、简报与巡看结果的提示，以及每个 Bot 的上次时间（落盘）。
import { join } from 'node:path'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'
import { botBriefText } from '../../prompts/bots.mjs'
import { pisperError } from '../../../shared/errors.mjs'

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

// 巡看子 agent 的任务 id：界面按它显示本地化的标题（“只读巡看”），运行时不写死某种语言的标题。
export const BOT_PROACTIVE_TASK_NAME = 'proactive_check'

// 巡看的交代、交回对话时的说明与简报的正文在 runtime/prompts/bots.mjs；简报前面加上隐藏标记。
export { BOT_PROACTIVE_NOTE, botProactiveTask } from '../../prompts/bots.mjs'

export function botBriefPrompt(options) {
  return [BOT_BRIEF_MARKER, botBriefText(options)].join('\n')
}

// 每个 Bot 上次巡看、上次简报的时间：<数据目录>/bots/<botId>/proactive.json。
export class BotProactiveStore {
  constructor({ dataDir }) {
    this.dataDir = dataDir
  }

  file(botId) {
    if (!BOT_ID_PATTERN.test(String(botId || ''))) throw pisperError('not_found', 'bot')
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
