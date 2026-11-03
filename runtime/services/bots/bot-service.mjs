// BotService：Bot 注册表的唯一权威（B1）。负责持久化、名字与默认聊天的全局唯一、目录解析，
// 以及删除前的占用检查。任务（BotTask）与触发在后续阶段接入，这里只通过 isBusy 回调询问。
import { randomBytes } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'
import {
  BotValidationError,
  LOCAL_ENVIRONMENT_ID,
  MAX_BOTS,
  botNameKey,
  normalizeBotInput,
} from './bot-model.mjs'

const STORE_VERSION = 1

export class BotServiceError extends Error {
  constructor(code, message, statusCode) {
    super(message)
    this.name = 'BotServiceError'
    this.code = code
    this.statusCode = statusCode
  }
}

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function newBotId() {
  return `bot_${randomBytes(9).toString('base64url')}`
}

export class BotService {
  constructor({ path, validateDirectory, isBusy = () => false, now = () => Date.now() }) {
    this.path = path
    this.validateDirectory = validateDirectory
    this.isBusy = isBusy
    this.now = now
    this.bots = []
    this.writeQueue = Promise.resolve()
  }

  // 读盘时逐条归一：坏条目与重名条目丢弃，不让一条坏数据拖垮整个注册表。
  async init() {
    const stored = await readJson(this.path, { version: STORE_VERSION, bots: [] })
    const bots = []
    const names = new Set()
    for (const value of Array.isArray(stored?.bots) ? stored.bots.slice(0, MAX_BOTS) : []) {
      try {
        const bot = normalizeBotInput(value)
        const key = botNameKey(bot.name)
        if (names.has(key) || typeof value.id !== 'string' || !value.id.startsWith('bot_')) continue
        names.add(key)
        bots.push({
          id: value.id,
          ...bot,
          createdAt: String(value.createdAt || ''),
          updatedAt: String(value.updatedAt || ''),
        })
      } catch {
        // 跳过这一条，其余照常加载。
      }
    }
    this.bots = bots
  }

  save() {
    const snapshot = { version: STORE_VERSION, bots: clone(this.bots) }
    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(() => writeJsonAtomic(this.path, snapshot, { mode: 0o600 }))
    return this.writeQueue
  }

  list() {
    return clone(this.bots)
  }

  get(id) {
    const bot = this.bots.find((item) => item.id === id)
    return bot ? clone(bot) : null
  }

  // 名字全局唯一；一个聊天最多一个默认 Bot（否则 @Pisper 不点名时不知道该交给谁）。
  #checkConflicts(bot, selfId = '') {
    const key = botNameKey(bot.name)
    if (this.bots.some((item) => item.id !== selfId && botNameKey(item.name) === key))
      throw new BotServiceError('bot_name_taken', `已经有叫“${bot.name}”的 Bot 了。`, 409)
    for (const chat of bot.defaultIn) {
      const owner = this.bots.find((item) => item.id !== selfId && item.defaultIn.includes(chat))
      if (owner)
        throw new BotServiceError(
          'bot_default_chat_taken',
          `这个聊天的默认 Bot 已经是“${owner.name}”。`,
          409,
        )
    }
  }

  async #resolveWorkspace(bot) {
    // B1 只有本机执行环境；执行节点在 B6 接入。
    if (bot.workspace.environmentId !== LOCAL_ENVIRONMENT_ID)
      throw new BotServiceError('environment_not_found', '执行环境不存在。', 400)
    try {
      bot.workspace.cwd = await this.validateDirectory(bot.workspace.cwd)
    } catch {
      throw new BotServiceError('directory_not_allowed', '工作目录不存在或不可访问。', 400)
    }
    return bot
  }

  async create(input) {
    if (this.bots.length >= MAX_BOTS)
      throw new BotServiceError('bot_limit', `最多可以有 ${MAX_BOTS} 个 Bot。`, 409)
    const bot = await this.#resolveWorkspace(normalizeBotInput(input))
    this.#checkConflicts(bot)
    const at = new Date(this.now()).toISOString()
    const created = { id: newBotId(), ...bot, createdAt: at, updatedAt: at }
    this.bots.push(created)
    await this.save()
    return clone(created)
  }

  async update(id, input) {
    const index = this.bots.findIndex((item) => item.id === id)
    if (index < 0) return null
    const current = this.bots[index]
    const bot = normalizeBotInput(input, current)
    // 目录变了才重新解析，避免每次改名字都去碰文件系统。
    if (
      bot.workspace.cwd !== current.workspace.cwd ||
      bot.workspace.environmentId !== current.workspace.environmentId
    )
      await this.#resolveWorkspace(bot)
    this.#checkConflicts(bot, id)
    const updated = {
      id,
      ...bot,
      createdAt: current.createdAt,
      updatedAt: new Date(this.now()).toISOString(),
    }
    this.bots[index] = updated
    await this.save()
    return clone(updated)
  }

  async remove(id) {
    const index = this.bots.findIndex((item) => item.id === id)
    if (index < 0) return false
    if (await this.isBusy(id))
      throw new BotServiceError(
        'invalid_state',
        '这个 Bot 还有进行中或待确认的任务，先停止或处理完再删除。',
        409,
      )
    this.bots.splice(index, 1)
    await this.save()
    return true
  }
}

export { BotValidationError }
