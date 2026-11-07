// Bots 接口：/api/bots。Bot 就是一个长期会话加上它的设定：在应用里和它说话走普通会话的接口，
// 这里只有注册表的增删改查与“打开它的会话”。响应按不可信数据校验，关键字段不对整体拒绝；
// 可选字段缺失时给安全的默认值，旧版 Runtime 多出或少了字段都不至于让页面崩掉。
import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'
import { fetchStartupQuery } from '@/lib/startup-queries'
import { parseWorkspaceFolderListing } from '@/lib/workspace-folders'
import { isBotCharacter, isBotColor, type BotAvatarValue } from './avatar'

// 此刻的动态：在不在忙（对话在说话或子 agent 在干活），在等你批准还是回答；
// working = 在做的子 agent 件数，current = 正在做的那件。
export type BotActivity = {
  status: 'running' | 'idle'
  waiting: 'approval' | 'question' | null
  needsYou: boolean
  working: number
  current: string
}

// 模型：按档位（跟着“子代理模型档位”的设置走）或指定 provider/model。
export const BOT_MODEL_TIERS = ['fast', 'standard', 'strong'] as const
export type BotModelTier = (typeof BOT_MODEL_TIERS)[number]
export type BotModel = { tier: BotModelTier } | { ref: string }
export const BOT_THINKING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh'] as const
export type BotThinking = (typeof BOT_THINKING_LEVELS)[number]

export type Bot = {
  id: string
  name: string
  avatar: BotAvatarValue
  enabled: boolean
  instructions: string
  executionMode: 'approval-required' | 'workspace-write' | 'full-access'
  model: BotModel
  // 思考等级；null 表示用模型默认。
  thinking: BotThinking | null
  // 渠道里哪些聊天、哪些人可以叫它（B3）。
  channels: BotChannel[]
  // 工具范围：null 表示用当前启用的全部工具。
  tools: string[] | null
  workspace: { environmentId: string; cwd: string }
  // 它的长期会话；新建 Bot 时就有，旧数据可能是空的（打开时补建）。
  threadSessionId: string
  activity: BotActivity | null
  // 同时跑几个子 agent（1–6，默认 3）。
  maxParallelAgents: number
  // 是否只读用户的全局记忆（默认开）。写入永远只写它自己的那份。
  readGlobalMemory: boolean
  // 主动（默认关）：空闲时每隔几小时只读巡看；每天定时写简报。
  proactive: BotProactive
}

export const BOT_PROACTIVE_HOURS = [1, 3, 6, 12, 24] as const
export type BotProactive = {
  enabled: boolean
  everyHours: (typeof BOT_PROACTIVE_HOURS)[number]
  brief: boolean
  briefAt: string
}

function parseProactive(value: unknown): BotProactive {
  const input = value && typeof value === 'object' ? (value as Record<string, unknown>) : {}
  const hours = BOT_PROACTIVE_HOURS.find((item) => item === input.everyHours) ?? 6
  return {
    enabled: input.enabled === true,
    everyHours: hours,
    brief: input.brief === true,
    briefAt:
      typeof input.briefAt === 'string' && /^\d{2}:\d{2}$/.test(input.briefAt)
        ? input.briefAt
        : '09:00',
  }
}

export type BotPlatform = 'feishu' | 'weixin' | 'qq' | 'telegram'
export const BOT_PLATFORMS: readonly BotPlatform[] = ['feishu', 'weixin', 'qq', 'telegram']

export type BotChannel = {
  platform: BotPlatform
  chatType: 'p2p' | 'group'
  peerIds: string[] | null
  senderIds: string[]
}

function parseChannels(value: unknown): BotChannel[] {
  if (!Array.isArray(value)) return []
  return value.flatMap((item) => {
    const input = item && typeof item === 'object' ? (item as Record<string, unknown>) : {}
    if (!BOT_PLATFORMS.includes(input.platform as BotPlatform)) return []
    const ids = (list: unknown) =>
      Array.isArray(list) ? list.filter((id): id is string => typeof id === 'string') : []
    return [
      {
        platform: input.platform as BotPlatform,
        chatType: input.chatType === 'group' ? 'group' : 'p2p',
        peerIds: Array.isArray(input.peerIds) ? ids(input.peerIds) : null,
        senderIds: ids(input.senderIds),
      },
    ]
  })
}

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalidResponseError()
  return value as Record<string, unknown>
}

function text(value: unknown, fallback = '') {
  return typeof value === 'string' ? value : fallback
}

function parseActivity(value: unknown): BotActivity | null {
  if (!value || typeof value !== 'object') return null
  const input = value as Record<string, unknown>
  const waiting =
    input.waiting === 'approval' || input.waiting === 'question' ? input.waiting : null
  const working = Number(input.working)
  return {
    status: input.status === 'running' ? 'running' : 'idle',
    waiting,
    needsYou: input.needsYou === true || waiting !== null,
    working: Number.isInteger(working) && working > 0 ? working : 0,
    current: typeof input.current === 'string' ? input.current.slice(0, 120) : '',
  }
}

export function parseBot(value: unknown): Bot {
  const input = object(value)
  const avatar = object(input.avatar ?? {})
  const workspace = object(input.workspace ?? {})
  if (typeof input.id !== 'string' || typeof input.name !== 'string') throw invalidResponseError()
  return {
    id: input.id,
    name: input.name,
    avatar: {
      character: isBotCharacter(avatar.character) ? avatar.character : 'blob',
      color: isBotColor(avatar.color) ? avatar.color : 'blue',
    },
    enabled: input.enabled !== false,
    instructions: text(input.instructions),
    executionMode:
      input.executionMode === 'workspace-write' || input.executionMode === 'full-access'
        ? input.executionMode
        : 'approval-required',
    model: parseModel(input.model),
    thinking: BOT_THINKING_LEVELS.find((level) => level === input.thinking) ?? null,
    channels: parseChannels(input.channels),
    tools: Array.isArray(input.tools)
      ? input.tools.filter((tool): tool is string => typeof tool === 'string')
      : null,
    workspace: {
      environmentId: text(workspace.environmentId, 'local'),
      cwd: text(workspace.cwd),
    },
    threadSessionId: text(input.threadSessionId),
    activity: parseActivity(input.activity),
    maxParallelAgents:
      Number.isInteger(input.maxParallelAgents) &&
      Number(input.maxParallelAgents) >= 1 &&
      Number(input.maxParallelAgents) <= 6
        ? Number(input.maxParallelAgents)
        : 3,
    readGlobalMemory: input.readGlobalMemory !== false,
    proactive: parseProactive(input.proactive),
  }
}

function parseModel(value: unknown): BotModel {
  const input = object(value ?? {})
  if (typeof input.ref === 'string' && input.ref.includes('/')) return { ref: input.ref }
  return { tier: BOT_MODEL_TIERS.find((tier) => tier === input.tier) ?? 'standard' }
}

export type BotModelOption = { ref: string; name: string; providerName: string }

// 设置页可选的具体模型：只列已配置且启用的服务商里的对话模型（与会话输入框的模型列表同一口径）。
// 配置走启动缓存，和会话页共用一份，不另发请求。
export async function fetchBotModelOptions(): Promise<BotModelOption[]> {
  const config = object(await fetchStartupQuery<unknown>('config'))
  const providers = Array.isArray(config.providers) ? config.providers : []
  return providers.flatMap((value) => {
    const provider = object(value)
    if (typeof provider.id !== 'string' || !provider.configured || provider.enabled === false)
      return []
    const providerName = text(provider.name, provider.id)
    const models = Array.isArray(provider.models) ? provider.models : []
    return models.flatMap((item) => {
      const model = object(item)
      if (typeof model.id !== 'string' || model.kind !== 'chat') return []
      return [{ ref: `${provider.id}/${model.id}`, name: text(model.name, model.id), providerName }]
    })
  })
}

function one<T>(key: string, parse: (value: unknown) => T) {
  return (value: unknown) => parse(object(value)[key])
}

export function fetchBots(signal?: AbortSignal) {
  return requestJson('/api/bots', {
    signal,
    parse: (value) => {
      const bots = object(value).bots
      if (!Array.isArray(bots)) throw invalidResponseError()
      return bots.map(parseBot)
    },
  })
}

export type BotDraft = {
  name: string
  avatar: BotAvatarValue
  instructions?: string
  workspace: { cwd: string }
}

export function createBot(draft: BotDraft) {
  return requestJson('/api/bots', { method: 'POST', data: draft, parse: one('bot', parseBot) })
}

export function updateBot(
  id: string,
  patch: Partial<Omit<Bot, 'id' | 'activity' | 'proactive'>> & {
    proactive?: Partial<BotProactive>
  },
) {
  return requestJson(`/api/bots/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    data: patch,
    parse: one('bot', parseBot),
  })
}

// 打开 Bot：它的长期会话 id（还没有就由 Runtime 新建），界面随后打开这个会话。
export function openBotSession(id: string) {
  return requestJson(`/api/bots/${encodeURIComponent(id)}/session`, {
    method: 'POST',
    data: {},
    parse: (value) => {
      const sessionId = object(value).sessionId
      if (typeof sessionId !== 'string' || !sessionId) throw invalidResponseError()
      return sessionId
    },
  })
}

// Bot 的电脑（B6）：本机与已配对的执行节点。
export type BotEnvironment = {
  id: string
  kind: 'local' | 'node'
  name: string
  online: boolean
  roots: string[]
  shell: boolean
  usedBy: { id: string; name: string }[]
  lastSeenAt: string | null
}

// 远程访问的地址与证书指纹：执行节点要连这里。没打开时节点连不上。
export type BotRemoteInfo = { enabled: boolean; endpoints: string[]; fingerprint: string | null }

function parseRemote(value: unknown): BotRemoteInfo {
  const input = value && typeof value === 'object' ? (value as Record<string, unknown>) : {}
  return {
    enabled: input.enabled === true,
    endpoints: Array.isArray(input.endpoints)
      ? input.endpoints.filter(
          (url): url is string => typeof url === 'string' && /^https:\/\//.test(url),
        )
      : [],
    fingerprint: typeof input.fingerprint === 'string' ? input.fingerprint : null,
  }
}

export function parseEnvironment(value: unknown): BotEnvironment {
  const input = object(value)
  if (typeof input.id !== 'string' || !input.id) throw invalidResponseError()
  const capabilities =
    input.capabilities && typeof input.capabilities === 'object'
      ? (input.capabilities as Record<string, unknown>)
      : {}
  return {
    id: input.id,
    kind: input.kind === 'node' ? 'node' : 'local',
    name: text(input.name),
    online: input.online === true,
    roots: Array.isArray(input.roots)
      ? input.roots.filter((root): root is string => typeof root === 'string')
      : [],
    shell: capabilities.shell === true,
    usedBy: Array.isArray(input.usedBy)
      ? input.usedBy.flatMap((item) => {
          const bot = item && typeof item === 'object' ? (item as Record<string, unknown>) : {}
          return typeof bot.id === 'string' && typeof bot.name === 'string'
            ? [{ id: bot.id, name: bot.name }]
            : []
        })
      : [],
    lastSeenAt: typeof input.lastSeenAt === 'string' ? input.lastSeenAt : null,
  }
}

export function fetchEnvironments(signal?: AbortSignal) {
  return requestJson('/api/environments', {
    signal,
    parse: (value) => {
      const input = object(value)
      if (!Array.isArray(input.environments)) throw invalidResponseError()
      return {
        environments: input.environments.map(parseEnvironment),
        remote: parseRemote(input.remote),
      }
    },
  })
}

export function createEnvironmentPairing() {
  return requestJson('/api/environments/pairing', {
    method: 'POST',
    data: {},
    parse: (value) => {
      const input = object(value)
      const pairing = object(input.pairing)
      if (typeof pairing.code !== 'string') throw invalidResponseError()
      return {
        code: pairing.code,
        expiresAt: text(pairing.expiresAt),
        remote: parseRemote(input.remote),
      }
    },
  })
}

// 在 Bot 的电脑上列一层子文件夹：执行节点上的目录由那台电脑回答，只在它允许的目录里。
export function listEnvironmentFolders(id: string, path: string, signal?: AbortSignal) {
  const query = path ? `?path=${encodeURIComponent(path)}` : ''
  return requestJson(`/api/environments/${encodeURIComponent(id)}/folders${query}`, {
    signal,
    parse: parseWorkspaceFolderListing,
  })
}

export function removeEnvironment(id: string) {
  return requestJson(`/api/environments/${encodeURIComponent(id)}`, {
    method: 'DELETE',
    parse: () => undefined,
  })
}

// 定时让 Bot 做的事（B3）：就是 targetType = bot 的定时任务，按 botId 过滤。
export type BotSchedule = {
  id: string
  enabled: boolean
  botMessage: string
  frequency: 'once' | 'interval' | 'daily' | 'weekly' | 'monthly'
  time: string
  date: string
  dayOfWeek: number
  nextRunAt: string | null
}

export function parseBotSchedules(value: unknown, botId: string): BotSchedule[] {
  const tasks = object(value).tasks
  if (!Array.isArray(tasks)) throw invalidResponseError()
  return tasks.flatMap((item) => {
    const task = item && typeof item === 'object' ? (item as Record<string, unknown>) : {}
    if (task.targetType !== 'bot' || task.botId !== botId || typeof task.id !== 'string') return []
    const frequency = ['once', 'interval', 'daily', 'weekly', 'monthly'].includes(
      String(task.frequency),
    )
      ? (task.frequency as BotSchedule['frequency'])
      : 'daily'
    return [
      {
        id: task.id,
        enabled: task.enabled !== false,
        botMessage: text(task.botMessage),
        frequency,
        time: text(task.time),
        date: text(task.date),
        dayOfWeek: Number(task.dayOfWeek) || 0,
        nextRunAt: typeof task.nextRunAt === 'string' ? task.nextRunAt : null,
      },
    ]
  })
}

export function fetchBotSchedules(botId: string, signal?: AbortSignal) {
  return requestJson('/api/schedules', {
    signal,
    parse: (value) => parseBotSchedules(value, botId),
  })
}

export type BotScheduleDraft = {
  botId: string
  name: string
  botMessage: string
  frequency: 'once' | 'daily' | 'weekly'
  time: string
  date: string
  dayOfWeek: number
  timezone: string
}

export function createBotSchedule(draft: BotScheduleDraft) {
  return requestJson('/api/schedules', {
    method: 'POST',
    data: { ...draft, targetType: 'bot', enabled: true },
    parse: () => undefined,
  })
}

export function setBotScheduleEnabled(id: string, enabled: boolean) {
  return requestJson(`/api/schedules/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    data: { enabled },
    parse: () => undefined,
  })
}

export function deleteBotSchedule(id: string) {
  return requestJson(`/api/schedules/${encodeURIComponent(id)}`, {
    method: 'DELETE',
    parse: () => undefined,
  })
}

// Bot 自己的记忆：一条一条的短句。设置页里看、加、改、删。
export type BotMemoryEntry = {
  id: string
  text: string
  source: 'told' | 'correction' | 'review' | 'proactive' | 'manual' | 'tidy'
  updatedAt: string
}

const MEMORY_SOURCES: readonly BotMemoryEntry['source'][] = [
  'told',
  'correction',
  'review',
  'proactive',
  'manual',
  'tidy',
]

function parseMemoryEntry(value: unknown): BotMemoryEntry | null {
  if (!value || typeof value !== 'object') return null
  const input = value as Record<string, unknown>
  if (typeof input.id !== 'string' || typeof input.text !== 'string' || !input.text) return null
  return {
    id: input.id,
    text: input.text.slice(0, 500),
    source: MEMORY_SOURCES.find((source) => source === input.source) ?? 'told',
    updatedAt: text(input.updatedAt),
  }
}

function oneEntry(value: unknown) {
  const entry = parseMemoryEntry(object(value).entry)
  if (!entry) throw invalidResponseError()
  return entry
}

export function fetchBotMemory(botId: string, signal?: AbortSignal) {
  return requestJson(`/api/bots/${encodeURIComponent(botId)}/memory`, {
    signal,
    parse: (value) => {
      const entries = object(value).entries
      return Array.isArray(entries)
        ? entries.map(parseMemoryEntry).filter((entry): entry is BotMemoryEntry => Boolean(entry))
        : []
    },
  })
}

export function addBotMemory(botId: string, memory: string) {
  return requestJson(`/api/bots/${encodeURIComponent(botId)}/memory`, {
    method: 'POST',
    data: { text: memory },
    parse: oneEntry,
  })
}

export function editBotMemory(botId: string, entryId: string, memory: string) {
  return requestJson(
    `/api/bots/${encodeURIComponent(botId)}/memory/${encodeURIComponent(entryId)}`,
    { method: 'PATCH', data: { text: memory }, parse: oneEntry },
  )
}

export function deleteBotMemory(botId: string, entryId: string) {
  return requestJson(
    `/api/bots/${encodeURIComponent(botId)}/memory/${encodeURIComponent(entryId)}`,
    { method: 'DELETE', parse: () => undefined },
  )
}

// Bot 自己写的技能（做完复盘写入）：设置页“它学会的”里看、删。
export type BotSkillSummary = {
  name: string
  description: string
  updatedAt: string
}

export type BotSkill = BotSkillSummary & { body: string }

function parseSkillSummary(value: unknown): BotSkillSummary | null {
  const input = object(value)
  if (typeof input.name !== 'string' || !input.name) return null
  return {
    name: input.name.slice(0, 64),
    description: text(input.description).slice(0, 1_024),
    updatedAt: text(input.updatedAt),
  }
}

export function fetchBotSkills(botId: string, signal?: AbortSignal) {
  return requestJson(`/api/bots/${encodeURIComponent(botId)}/skills`, {
    signal,
    parse: (value) => {
      const skills = object(value).skills
      return Array.isArray(skills)
        ? skills.map(parseSkillSummary).filter((skill): skill is BotSkillSummary => Boolean(skill))
        : []
    },
  })
}

export function fetchBotSkill(botId: string, name: string, signal?: AbortSignal) {
  return requestJson(`/api/bots/${encodeURIComponent(botId)}/skills/${encodeURIComponent(name)}`, {
    signal,
    parse: (value): BotSkill => {
      const skill = object(object(value).skill)
      const summary = parseSkillSummary(skill)
      if (!summary) throw new Error('Invalid skill response.')
      return { ...summary, body: text(skill.body).slice(0, 20_000) }
    },
  })
}

export function deleteBotSkill(botId: string, name: string) {
  return requestJson(`/api/bots/${encodeURIComponent(botId)}/skills/${encodeURIComponent(name)}`, {
    method: 'DELETE',
    parse: () => undefined,
  })
}
