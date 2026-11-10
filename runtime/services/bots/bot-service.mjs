// BotService：Bot 注册表的唯一权威（B1）。负责持久化、名字与默认聊天的全局唯一、目录解析，
// 以及删除前的占用检查。任务（BotTask）与触发在后续阶段接入，这里只通过 isBusy 回调询问。
import { randomBytes } from 'node:crypto'
import { pathWithinRoots } from './environment-service.mjs'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'
import { pisperError } from '../../../shared/errors.mjs'
import { LOCAL_ENVIRONMENT_ID, MAX_BOTS, botNameKey, normalizeBotInput } from './bot-model.mjs'

const STORE_VERSION = 1

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function newBotId() {
  return `bot_${randomBytes(9).toString('base64url')}`
}

export class BotService {
  constructor({
    path,
    validateDirectory,
    // 执行节点（B6）：按 id 取已配对的节点；没有时只认本机。
    getEnvironment = () => null,
    isBusy = () => false,
    now = () => Date.now(),
  }) {
    this.path = path
    this.getEnvironment = getEnvironment
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
          threadSessionId:
            typeof value.threadSessionId === 'string' ? value.threadSessionId.slice(0, 200) : '',
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

  // 这个会话是哪个 Bot 的长期对话。每次权限判断、看板上每张卡都会问：只拷贝找到的那一个，
  // 不走 list()（它把全部 Bot 连同指令深拷贝一遍）。
  forSession(sessionId) {
    if (!sessionId) return null
    const bot = this.bots.find((item) => item.threadSessionId === sessionId)
    return bot ? clone(bot) : null
  }

  // 名字全局唯一；一个聊天最多一个默认 Bot（否则 @Pisper 不点名时不知道该交给谁）。
  #checkConflicts(bot, selfId = '') {
    const key = botNameKey(bot.name)
    if (this.bots.some((item) => item.id !== selfId && botNameKey(item.name) === key))
      throw pisperError('exists', 'bot', bot.name)
    for (const chat of bot.defaultIn) {
      const owner = this.bots.find((item) => item.id !== selfId && item.defaultIn.includes(chat))
      if (owner) throw pisperError('exists', 'bot', `default bot of this chat is ${owner.name}`)
    }
  }

  async #resolveWorkspace(bot) {
    // 执行节点上的目录在节点那边按真实路径检查；这里只要求是它允许的目录之一里的绝对路径。
    if (bot.workspace.environmentId !== LOCAL_ENVIRONMENT_ID) {
      const environment = this.getEnvironment(bot.workspace.environmentId)
      if (!environment) throw pisperError('not_found', 'machine')
      if (!pathWithinRoots(bot.workspace.cwd, environment.roots))
        throw pisperError('forbidden', 'folder', "not inside the machine's allowed folders", {
          status: 400,
        })
      return bot
    }
    try {
      bot.workspace.cwd = await this.validateDirectory(bot.workspace.cwd)
    } catch {
      throw pisperError('not_found', 'folder', 'missing or not accessible', { status: 400 })
    }
    return bot
  }

  async create(input) {
    if (this.bots.length >= MAX_BOTS)
      throw pisperError('limit', 'bot', `max ${MAX_BOTS}`, { status: 409 })
    const bot = await this.#resolveWorkspace(normalizeBotInput(input))
    this.#checkConflicts(bot)
    const at = new Date(this.now()).toISOString()
    const created = { id: newBotId(), ...bot, threadSessionId: '', createdAt: at, updatedAt: at }
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
      threadSessionId: current.threadSessionId || '',
      createdAt: current.createdAt,
      updatedAt: new Date(this.now()).toISOString(),
    }
    this.bots[index] = updated
    await this.save()
    return clone(updated)
  }

  // 记下 Bot 的长期会话：它的所有任务都在这个会话里跑，对话页据此打开完整记录。
  async setThread(id, sessionId) {
    const bot = this.bots.find((item) => item.id === id)
    if (!bot || !sessionId || bot.threadSessionId === sessionId) return
    bot.threadSessionId = String(sessionId).slice(0, 200)
    await this.save()
  }

  async remove(id) {
    const index = this.bots.findIndex((item) => item.id === id)
    if (index < 0) return false
    if (await this.isBusy(id))
      throw pisperError('busy', 'bot', 'stop or finish its jobs and approvals first')
    this.bots.splice(index, 1)
    await this.save()
    return true
  }
}
