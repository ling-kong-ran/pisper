// Bot 的对话（v2.1）：对话自己做快的事（读、查、小改、短命令），长的、多步的、要批准的交给子 agent 在后台并行跑；
// 运行时保证它不阻塞（新消息插进来、长命令转后台、子 agent 能做的不在对话里等批准）。它当经理用的手是 bot_* 工具。
// 这里是纯函数：bot_* 工具输入的归一、子 agent 引用的解析、复盘结果的解析；给模型看的文字（系统提示、状态快照、
// 复盘交代）在 runtime/prompts/bots.mjs，这里按原名转出。运行时接线在 agent-runtime-facade。
import { botStatePrompt } from '../../prompts/bots.mjs'

export { botConversationPrompt, botReviewInstruction } from '../../prompts/bots.mjs'

export const BOT_MAX_PARALLEL_AGENTS = 3
const MAX_TITLE_CHARS = 80
const MAX_TASK_CHARS = 8_000
const MAX_ADD_CHARS = 4_000
const TIERS = new Set(['fast', 'standard', 'strong'])
const MEMORY_SOURCES = new Set(['told', 'correction', 'review', 'proactive'])
const MAX_MEMORY_CHARS = 500
const MAX_MESSAGE_CHARS = 4_000

// 每轮附带的状态快照：默认上限取这里的常量。
export function botStateSnapshot({ limit = BOT_MAX_PARALLEL_AGENTS, ...state } = {}) {
  return botStatePrompt({ ...state, limit })
}

function record(value) {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : {}
}

function text(value, max) {
  const result = String(value ?? '').trim()
  return result && result.length <= max ? result : ''
}

// bot_* 工具的输入 → 运行时要执行的动作；不合法时为 null（工具据此报错，不执行）。
export function normalizeBotAction(input) {
  const action = record(input)
  if (action.type === 'spawn') {
    const title = text(action.title, MAX_TITLE_CHARS)
    const task = text(action.task, MAX_TASK_CHARS)
    if (!title || !task) return null
    const model =
      typeof action.model === 'string' && action.model.includes('/') ? action.model.trim() : ''
    const tier = TIERS.has(String(action.tier)) ? String(action.tier) : ''
    return { type: 'spawn', title, task, ...(model ? { model } : {}), ...(tier ? { tier } : {}) }
  }
  if (action.type === 'add') {
    const agent = text(action.agent, 64)
    const addText = text(action.text, MAX_ADD_CHARS)
    return agent && addText ? { type: 'add', agent, text: addText } : null
  }
  if (action.type === 'stop') {
    const agent = text(action.agent, 64)
    return agent ? { type: 'stop', agent } : null
  }
  if (action.type === 'remember') {
    const memory = text(action.text, MAX_MEMORY_CHARS)
    const source = MEMORY_SOURCES.has(String(action.source)) ? String(action.source) : 'told'
    if (!memory) return null
    return {
      type: 'remember',
      text: memory,
      source,
      ...(action.again === true ? { again: true } : {}),
    }
  }
  if (action.type === 'forget') {
    const memory = text(action.memory, 64)
    return memory ? { type: 'forget', memory } : null
  }
  if (action.type === 'message') {
    const bot = text(action.bot, 64)
    const messageText = text(action.text, MAX_MESSAGE_CHARS)
    return bot && messageText
      ? { type: 'message', bot, text: messageText, expectsReply: action.expectsReply !== false }
      : null
  }
  return null
}

// 决定里的子 agent 引用（id 前缀）→ 这个 Bot 自己的子 agent；对不上或有歧义时为空，不碰别人的。
export function resolveBotAgent(agents, reference) {
  const prefix = String(reference || '').trim()
  if (!prefix) return null
  const matches = agents.filter((agent) => agent.id.startsWith(prefix))
  return matches.length === 1 ? matches[0] : null
}

// 解析复盘结果：{"skill": {...}} 才算要写；格式不对、名字不合法、正文为空时当没有（复盘失败不影响别的）。
export function parseBotSkillReview(raw) {
  const source = String(raw || '')
  const start = source.indexOf('{')
  const end = source.lastIndexOf('}')
  if (start < 0 || end <= start) return null
  let parsed
  try {
    parsed = JSON.parse(source.slice(start, end + 1))
  } catch {
    return null
  }
  const skill = record(record(parsed).skill)
  const name = String(skill.name || '').trim()
  const description = String(skill.description || '').trim()
  const body = String(skill.body || '').trim()
  if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name) || name.length > 64) return null
  if (!description || !body) return null
  return { name, description: description.slice(0, 1_024), body: body.slice(0, 20_000) }
}
