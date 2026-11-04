// Bot 的数据模型：把不可信输入（HTTP 请求体、磁盘上的旧数据）校验并归一化成 Bot 记录。
// 只做纯校验，不碰文件与目录；目录解析、名字唯一、默认聊天唯一由 BotService 负责。
// 字段含义见本目录 README 的“概念模型”。
import { normalizeExecutionMode } from '../../security/execution-mode.mjs'
import { SUBAGENT_MODEL_TIERS } from '../agent-profiles.mjs'

export const MAX_BOTS = 32
export const MAX_BOT_NAME_CHARS = 24
export const MAX_BOT_INSTRUCTIONS_CHARS = 12_000
export const BOT_PLATFORMS = Object.freeze(['feishu', 'weixin', 'qq', 'telegram'])
export const BOT_RUN_MODES = Object.freeze(['plan', 'goal', 'team'])
// 头像 = 角色 + 颜色，与前端 src/features/bots/avatar.ts 的角色表一致（动态 SVG 在前端生成）。
export const BOT_AVATAR_CHARACTERS = Object.freeze([
  'blob',
  'bean',
  'robot',
  'cat',
  'owl',
  'ghost',
  'cloud',
  'octo',
  'bunny',
  'bear',
  'fox',
  'star',
])
export const BOT_AVATAR_COLORS = Object.freeze([
  'blue',
  'green',
  'orange',
  'purple',
  'teal',
  'pink',
])
export const LOCAL_ENVIRONMENT_ID = 'local'
const THINKING_LEVELS = new Set(['off', 'minimal', 'low', 'medium', 'high', 'xhigh'])
const MAX_CHANNELS = 8
const MAX_IDS = 50
const MAX_ID_CHARS = 200
const MAX_TOOLS = 64
const MAX_MODEL_REF_CHARS = 300
// 名字会出现在“@Pisper 审查员 …”“/审查员 …”里：只允许字母（含中文）、数字、下划线与连字符，不能有空白。
const NAME_PATTERN = /^[\p{L}\p{N}_-]+$/u

export class BotValidationError extends Error {
  constructor(code, message) {
    super(message)
    this.name = 'BotValidationError'
    this.code = code
    this.statusCode = 400
  }
}

function invalid(code, message) {
  return new BotValidationError(code, message)
}

// Bot 会话的系统提示追加段：它是谁、它的指令。每次重建会话运行时都按当前设定生成，改了指令下一轮就生效。
export function botSystemPrompt(bot) {
  const parts = [
    `你是 ${bot.name}，用户在 Pisper 里的一个长期助手。这是你和用户的一段持续对话：前文都算数，消息也可能来自用户的聊天软件或定时任务。`,
    // 用户把事交给 Bot 是为了不用盯着：能自己做完的就做完，别为了确认而停下来。
    '能自己查到、自己做完的事直接做完，不要为了确认而停下来问；只有缺少只有用户知道的信息、要用户拍板，或者你确实做不了时，才问用户。',
  ]
  const instructions = String(bot.instructions || '').trim()
  if (instructions) parts.push(`[${bot.name} 的工作说明]\n${instructions}`)
  return parts.join('\n\n')
}

export const DEFAULT_BOT_EXECUTION_MODE = 'workspace-write'

export const MAX_BOT_MESSAGE_CHARS = 12_000

// 渠道、定时任务交给 Bot 的一句话（应用里直接在 Bot 的会话里说，走普通会话的校验）。
export function normalizeBotMessage(value) {
  const text = String(value ?? '').trim()
  if (!text) throw invalid('bot_message_empty', '要让 Bot 做什么？请写下来。')
  if (text.length > MAX_BOT_MESSAGE_CHARS)
    throw invalid('bot_message_too_long', `一次最多 ${MAX_BOT_MESSAGE_CHARS} 个字符。`)
  return text
}

/** @param {unknown} value @returns {Record<string, unknown>} */
function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? /** @type {Record<string, unknown>} */ (value)
    : {}
}

// 名字比较用的键：NFKC 归一 + 小写，“审查员”与全角、大小写变体视为同名。
export function botNameKey(name) {
  return String(name || '')
    .normalize('NFKC')
    .toLowerCase()
}

function normalizeName(value) {
  const name = String(value ?? '')
    .normalize('NFKC')
    .trim()
  if (!name) throw invalid('bot_name_required', 'Bot 名字不能为空。')
  if ([...name].length > MAX_BOT_NAME_CHARS)
    throw invalid('bot_name_too_long', `Bot 名字最多 ${MAX_BOT_NAME_CHARS} 个字符。`)
  if (!NAME_PATTERN.test(name))
    throw invalid('bot_name_invalid', 'Bot 名字只能包含文字、数字、下划线和连字符，不能有空格。')
  return name
}

// 未指定或旧数据（一个字 + 1–6 号颜色）按名字稳定地挑一个角色，颜色序号映射到颜色名。
function normalizeAvatar(value, name) {
  const input = record(value)
  let seed = 0
  for (const char of name) seed = (seed * 31 + char.codePointAt(0)) % 9973
  const character = BOT_AVATAR_CHARACTERS.includes(String(input.character))
    ? String(input.character)
    : BOT_AVATAR_CHARACTERS[seed % BOT_AVATAR_CHARACTERS.length]
  const legacyColor = Number(input.color)
  const color = BOT_AVATAR_COLORS.includes(String(input.color))
    ? String(input.color)
    : Number.isInteger(legacyColor) && legacyColor >= 1 && legacyColor <= 6
      ? BOT_AVATAR_COLORS[legacyColor - 1]
      : BOT_AVATAR_COLORS[seed % BOT_AVATAR_COLORS.length]
  return { character, color }
}

function normalizeModel(value) {
  const input = record(value)
  if (typeof input.ref === 'string' && input.ref.trim()) {
    const ref = input.ref.trim()
    if (ref.length > MAX_MODEL_REF_CHARS || !ref.includes('/'))
      throw invalid('bot_model_invalid', '模型必须写成 provider/model。')
    return { ref }
  }
  const tier = SUBAGENT_MODEL_TIERS.includes(String(input.tier)) ? String(input.tier) : 'standard'
  return { tier }
}

function normalizeIdList(value, label) {
  if (value === null || value === undefined) return []
  if (!Array.isArray(value)) throw invalid('bot_channel_invalid', `${label}必须是列表。`)
  const ids = [
    ...new Set(
      value
        .map((item) => String(item ?? '').trim())
        .filter((item) => item && item.length <= MAX_ID_CHARS),
    ),
  ]
  if (ids.length > MAX_IDS) throw invalid('bot_channel_invalid', `${label}最多 ${MAX_IDS} 个。`)
  return ids
}

function normalizeChannels(value) {
  if (value === null || value === undefined) return []
  if (!Array.isArray(value)) throw invalid('bot_channel_invalid', '渠道必须是列表。')
  if (value.length > MAX_CHANNELS)
    throw invalid('bot_channel_invalid', `渠道最多 ${MAX_CHANNELS} 个。`)
  return value.map((item) => {
    const input = record(item)
    const platform = String(input.platform || '')
    if (!BOT_PLATFORMS.includes(platform))
      throw invalid('bot_channel_invalid', `不支持的渠道：${platform.slice(0, 40)}`)
    const chatType = input.chatType === 'group' ? 'group' : 'p2p'
    // peerIds 为 null 表示该平台所有已允许的聊天；空数组同样按“不限”处理会造成误解，统一成 null。
    const peerIds =
      input.peerIds === null || input.peerIds === undefined
        ? null
        : normalizeIdList(input.peerIds, '聊天')
    const senderIds = normalizeIdList(input.senderIds, '可以叫它的人')
    // 群里谁都能发消息：必须写明谁可以叫它，否则群成员（或被拉进群的人）都能让 Bot 干活。
    if (chatType === 'group' && senderIds.length === 0)
      throw invalid('bot_group_needs_senders', '群聊必须写明哪些人可以叫这个 Bot。')
    return { platform, chatType, peerIds: peerIds && peerIds.length ? peerIds : null, senderIds }
  })
}

function normalizeDefaultIn(value, channels) {
  const keys = normalizeIdList(value, '默认聊天')
  for (const key of keys) {
    const separator = key.indexOf(':')
    const platform = key.slice(0, separator)
    const peerId = key.slice(separator + 1)
    const covered =
      separator > 0 &&
      peerId &&
      channels.some(
        (channel) =>
          channel.platform === platform &&
          (channel.peerIds === null || channel.peerIds.includes(peerId)),
      )
    if (!covered) throw invalid('bot_default_chat_invalid', '默认聊天必须在这个 Bot 的渠道范围内。')
  }
  return keys
}

export const DEFAULT_BOT_PARALLEL_AGENTS = 3
export const MAX_BOT_PARALLEL_AGENTS = 6
export const BOT_PROACTIVE_HOURS = Object.freeze([1, 3, 6, 12, 24])

// 同时跑几个子 agent：1–6，默认 3（多的排队）。
function normalizeParallel(value) {
  const count = Number(value)
  return Number.isInteger(count) && count >= 1 && count <= MAX_BOT_PARALLEL_AGENTS
    ? count
    : DEFAULT_BOT_PARALLEL_AGENTS
}

// 主动（默认关）：空闲时每隔几小时做一次只读巡看；每天在设定时间写一份简报。两者各自开关。
function normalizeProactive(value) {
  const input = record(value)
  const hours = Number(input.everyHours)
  const briefAt = /^([01]\d|2[0-3]):[0-5]\d$/.test(String(input.briefAt))
    ? String(input.briefAt)
    : '09:00'
  return {
    enabled: input.enabled === true,
    everyHours: BOT_PROACTIVE_HOURS.includes(hours) ? hours : 6,
    brief: input.brief === true,
    briefAt,
  }
}

function normalizeTools(value) {
  if (value === null || value === undefined) return null
  if (!Array.isArray(value)) throw invalid('bot_tools_invalid', '工具必须是列表。')
  const tools = [...new Set(value.map((item) => String(item ?? '').trim()).filter(Boolean))]
  if (tools.length > MAX_TOOLS || tools.some((tool) => tool.length > 100))
    throw invalid('bot_tools_invalid', `工具最多 ${MAX_TOOLS} 个。`)
  return tools
}

// 归一化输入。current 为修改前的记录（新建时为 null）；未提供的字段沿用 current。
// 目录（workspace.cwd）在这里只做形状检查，真实解析在 BotService 里异步完成。
export function normalizeBotInput(input, current = null) {
  const patch = record(input)
  const merged = { ...(current || {}), ...patch }
  const name = normalizeName(merged.name)
  const instructions = String(merged.instructions ?? '')
  if (instructions.length > MAX_BOT_INSTRUCTIONS_CHARS)
    throw invalid('bot_instructions_too_long', `指令最多 ${MAX_BOT_INSTRUCTIONS_CHARS} 个字符。`)
  const workspaceInput = { ...record(current?.workspace), ...record(patch.workspace) }
  const environmentId = String(workspaceInput.environmentId || LOCAL_ENVIRONMENT_ID)
  if (environmentId !== LOCAL_ENVIRONMENT_ID && !/^env_[A-Za-z0-9_-]{8,32}$/.test(environmentId))
    throw invalid('environment_not_found', '这台电脑不存在或已经移除。')
  const cwd = String(workspaceInput.cwd ?? '').trim()
  if (!cwd) throw invalid('bot_cwd_required', '请为 Bot 选择工作目录。')
  const channels = normalizeChannels(merged.channels)
  // Bot 是替用户干活的：新建默认放手做（工作区写入档），只有命令守卫标出的危险操作、浏览器提交、
  // 第三方插件这类事才停下来问。已经存下的 Bot 保留它自己的权限，不悄悄放宽。
  const executionMode = normalizeExecutionMode(merged.executionMode, DEFAULT_BOT_EXECUTION_MODE)
  // 执行权限是上限：有群聊触发时不能是完全访问，否则群里一句话就能让 Bot 不经确认执行任何命令。
  if (executionMode === 'full-access' && channels.some((channel) => channel.chatType === 'group'))
    throw invalid('bot_group_full_access', '有群聊触发的 Bot 不能使用完全访问权限。')
  const thinking = THINKING_LEVELS.has(String(merged.thinking))
    ? String(merged.thinking)
    : undefined
  return {
    name,
    avatar: normalizeAvatar(merged.avatar, name),
    enabled: merged.enabled !== false,
    instructions,
    model: normalizeModel(merged.model),
    ...(thinking ? { thinking } : {}),
    runMode: BOT_RUN_MODES.includes(String(merged.runMode)) ? String(merged.runMode) : 'plan',
    // Bot 就是一个长期会话加上它的设定：会话在这台电脑的这个目录里跑，不依赖 git、不开分支。
    workspace: { environmentId, cwd },
    executionMode,
    tools: normalizeTools(merged.tools),
    channels,
    defaultIn: normalizeDefaultIn(merged.defaultIn, channels),
    maxParallelAgents: normalizeParallel(merged.maxParallelAgents),
    // 只改其中一项时保留其余几项。
    proactive: normalizeProactive({ ...record(current?.proactive), ...record(patch.proactive) }),
  }
}
