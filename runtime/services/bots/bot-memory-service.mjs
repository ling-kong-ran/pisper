// 每个 Bot 的记忆：记忆引擎里的一个独立空间（bot:<id>），这里只是引擎上的薄适配。
// 对外仍是一条一条的短句 { id, text, source, createdAt, updatedAt }，设置页和 bot_* 工具不用变。
// 隔离靠空间：读写都按这个 Bot 的空间走，改、删前先核对条目属于它，拿别的 Bot 的 id 也碰不到。
// Bot 只写自己的空间；全局记忆默认只读（readGlobalMemory），Bot 之间互不可见。
import { randomBytes } from 'node:crypto'
import { rm } from 'node:fs/promises'
import { join } from 'node:path'
import { readJson } from '../../storage/json-file.mjs'
import { botSpaceId } from '../memory/local-memory-runtime.mjs'
import { pisperError } from '../../../shared/errors.mjs'

export const MAX_BOT_MEMORY_CHARS = 500
export const BOT_MEMORY_SOURCES = Object.freeze([
  'told',
  'correction',
  'review',
  'proactive',
  'manual',
  'tidy',
])
const BOT_ID_PATTERN = /^bot_[A-Za-z0-9_-]{6,40}$/
const ENTRY_ID_PATTERN = /^mem_[A-Za-z0-9_-]{6,32}$/
const TITLE_CHARS = 60
const LIST_PAGE = 300
const PROFILE_TAG = '<pisper_memory_profile>'
const NO_CONTEXT = Object.freeze({ text: '', memories: [] })

function normalizeText(value) {
  const text = String(value ?? '')
    .replace(/\s+/g, ' ')
    .trim()
  if (!text) throw pisperError('invalid', 'memory', 'text is required')
  if (text.length > MAX_BOT_MEMORY_CHARS)
    throw pisperError('too_large', 'memory', `max ${MAX_BOT_MEMORY_CHARS} characters`)
  return text
}

// 比较用的键：去掉空白与标点、统一大小写，“以后别 push 到 main。”与“以后别push到main”算同一条。
function sameKey(text) {
  return String(text || '')
    .normalize('NFKC')
    .toLowerCase()
    .replace(/[\s\p{P}]+/gu, '')
}

// 这一轮的话是谁说的。应用里的用户、用户本人绑定的渠道账号、问题卡片的回答算用户；
// 定时任务没人在场，不能自称“用户说的”；别的 Bot 和渠道里的其他人算外部，其余拿不准的也算外部。
export function botSourceTrust(source) {
  const kind = source?.kind
  if (kind === 'chat' || kind === 'answer') return 'user'
  if (kind === 'channel' && source.fromOwner === true) return 'user'
  if (kind === 'schedule') return 'agent'
  return 'external'
}

const TRUST_ORDER = ['user', 'agent', 'external']

// 一轮里插进来多句话时，按最不可信的那句算。
export function stricterBotTrust(left, right) {
  return TRUST_ORDER[
    Math.max(TRUST_ORDER.indexOf(left || 'user'), TRUST_ORDER.indexOf(right || 'user'))
  ]
}

function engineSource(kind, trust) {
  if (kind === 'manual') return { sourceType: 'manual', evidenceRole: 'manual' }
  if (trust === 'external') return { sourceType: 'external', evidenceRole: 'external' }
  // 复盘、巡看是它自己的结论；用户让记的、纠正的才算用户的话。
  if (trust === 'agent' || kind === 'review' || kind === 'proactive')
    return { sourceType: 'agent', evidenceRole: 'assistant' }
  return { sourceType: 'conversation', evidenceRole: 'user' }
}

function entryFrom(memory) {
  const kept = String(memory.sourceId || '').replace(/^bot:/, '')
  const confirmed = memory.lastConfirmedAt || ''
  return {
    id: memory.id,
    text: memory.content,
    source: BOT_MEMORY_SOURCES.includes(kept) ? kept : 'told',
    createdAt: memory.validFrom || memory.createdAt,
    updatedAt: confirmed > memory.updatedAt ? confirmed : memory.updatedAt,
  }
}

function legacyEntry(value) {
  if (!ENTRY_ID_PATTERN.test(String(value?.id || ''))) return null
  const text = String(value.text ?? '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, MAX_BOT_MEMORY_CHARS)
  if (!text) return null
  return {
    id: value.id,
    text,
    source: BOT_MEMORY_SOURCES.includes(value.source) ? value.source : 'told',
    validFrom: typeof value.createdAt === 'string' ? value.createdAt : undefined,
  }
}

export class BotMemoryService {
  constructor({ dataDir, memory, waitForMemory = async () => {}, getBotName = () => '' }) {
    this.dataDir = dataDir
    this.memory = memory
    this.waitForMemory = waitForMemory
    this.getBotName = getBotName
    this.imports = new Map()
  }

  legacyFile(botId) {
    if (!BOT_ID_PATTERN.test(String(botId || ''))) throw pisperError('not_found', 'bot')
    return join(this.dataDir, 'bots', botId, 'memory.json')
  }

  // 这个 Bot 的空间。手机上记忆是延迟初始化的，先等；这个环境用不了记忆时给结构化错误。
  async space(botId) {
    this.legacyFile(botId)
    try {
      await this.waitForMemory()
    } catch {
      throw this.unavailable()
    }
    if (!this.memory?.db) throw this.unavailable()
    const spaceId = this.memory.ensureBotSpace(botId, this.getBotName(botId))
    await this.importLegacy(botId, spaceId)
    return spaceId
  }

  unavailable() {
    return pisperError('unavailable', 'memory', undefined, { status: 503 })
  }

  // 旧版把记忆存在 memory.json：第一次用到时导入一次，然后删掉文件。
  // 导入到一半崩溃时文件还在，下次重来；引擎按内容去重，不会多出来。
  importLegacy(botId, spaceId) {
    let task = this.imports.get(botId)
    if (!task) {
      task = this.readLegacy(botId, spaceId).catch(() =>
        console.warn('[bots] Could not import the old memory file; will retry next start.'),
      )
      this.imports.set(botId, task)
    }
    return task
  }

  async readLegacy(botId, spaceId) {
    const file = this.legacyFile(botId)
    const state = await readJson(file, null)
    if (!state) return
    for (const entry of (Array.isArray(state.entries) ? state.entries : []).map(legacyEntry)) {
      if (entry)
        this.write(spaceId, botId, entry.text, entry.source, {
          id: entry.id,
          validFrom: entry.validFrom,
        })
    }
    await rm(file, { force: true })
  }

  write(spaceId, botId, text, kind, { id, validFrom, trust, again = false } = {}) {
    const memoryId = id || `mem_${randomBytes(9).toString('base64url')}`
    return this.memory.remember({
      id: memoryId,
      spaceId,
      title: text.slice(0, TITLE_CHARS),
      content: text,
      type: kind === 'correction' ? 'preference' : 'fact',
      // 每条一个主题：两条开头相同的话不能因为标题一样就互相替换，矛盾交给对账判断。
      topic: `bot.memory.${memoryId}`,
      ...engineSource(kind, trust),
      sourceId: `bot:${kind}`,
      actor: `bot:${botId}`,
      validFrom,
      // 用户删过的内容不再记。设置页亲手加，或用户刚刚又要求记住，才解除。
      ignoreForgotten: kind === 'manual' || again === true,
    })
  }

  // 当前有效的全部条目，从旧到新。分页只是因为引擎单页有上限：这里不截断。
  current(spaceId) {
    const rows = new Map()
    let before = ''
    for (;;) {
      const page = this.memory.listMemories({ spaceId, limit: LIST_PAGE, before })
      const known = rows.size
      for (const row of page) rows.set(row.id, row)
      if (page.length < LIST_PAGE || rows.size === known) break
      // 加 1 毫秒：同一毫秒的条目可能跨页，宁可多取一点，再按 id 去重。
      before = new Date(Date.parse(page.at(-1).validFrom) + 1).toISOString()
    }
    return [...rows.values()].sort((left, right) =>
      String(left.validFrom).localeCompare(String(right.validFrom)),
    )
  }

  async list(botId) {
    return this.current(await this.space(botId)).map(entryFrom)
  }

  // 记住一条：和已有的某条说的是一回事时只刷新证实时间，不重复记。
  // options.trust 是这一轮的话是谁说的（见 botSourceTrust）；用户删过时返回 forgotten: true。
  async remember(botId, text, source = 'told', options = {}) {
    const value = normalizeText(text)
    const kind = BOT_MEMORY_SOURCES.includes(source) ? source : 'told'
    const spaceId = await this.space(botId)
    const key = sameKey(value)
    const existing = this.current(spaceId).find((row) => sameKey(row.content) === key)
    const saved = this.write(spaceId, botId, existing ? existing.content : value, kind, options)
    if (saved.outcome === 'forgotten')
      return { id: '', text: value, source: kind, createdAt: '', updatedAt: '', forgotten: true }
    return entryFrom(saved)
  }

  // 属于这个 Bot、当前有效的那一条；别的 Bot 的 id 查不到。
  own(spaceId, id) {
    const memory = id ? this.memory.getMemory(id) : null
    if (!memory || memory.spaceId !== spaceId) return null
    return memory.status === 'active' || memory.status === 'stale' ? memory : null
  }

  async edit(botId, id, text) {
    const value = normalizeText(text)
    const spaceId = await this.space(botId)
    if (!this.own(spaceId, id)) throw pisperError('not_found', 'memory')
    return entryFrom(
      this.memory.updateMemory(id, { title: value.slice(0, TITLE_CHARS), content: value }),
    )
  }

  // 忘掉：按 id，或者按内容（对话里说“忘掉 X”时）。找不到返回 null。
  async forget(botId, { id = '', text = '' } = {}) {
    const spaceId = await this.space(botId)
    const key = text ? sameKey(text) : ''
    const found = id
      ? this.own(spaceId, id)
      : key
        ? this.current(spaceId).find((row) => sameKey(row.content) === key)
        : null
    if (!found) return null
    this.memory.forget(found.id, 'user_deleted')
    return entryFrom(found)
  }

  // 画像常驻用的空间：自己的，加上开了开关时只读的全局。
  readableSpaces(bot, spaceId) {
    return bot.readGlobalMemory === false ? [spaceId] : [spaceId, 'global']
  }

  // 每轮随用户消息注入的记忆。画像只在首轮和压缩后带一次（压缩把它总结掉后，历史里就没有了）；
  // 相关的事实每轮按这句话检索，只搜自己的空间。记忆用不了时不影响对话。
  async turnContext(bot, { message, cwd = '', userTexts = [], excludeIds = [], limit } = {}) {
    try {
      const spaceId = await this.space(bot.id)
      const profile = userTexts.some((text) => text.includes(PROFILE_TAG))
        ? ''
        : this.memory.profileContext(this.readableSpaces(bot, spaceId))
      const recall = await this.memory.relevantContext(message, cwd, limit, {
        spaceIds: [spaceId],
        excludeIds,
      })
      return {
        text: [profile, recall.text].filter(Boolean).join('\n\n'),
        memories: recall.memories,
      }
    } catch {
      return NO_CONTEXT
    }
  }

  // 派给子 agent 的背景：它不记事，所以每次都带画像，再按这件活的内容检索相关的事实。
  async agentContext(bot, { task = '', cwd = '' } = {}) {
    const { text } = await this.turnContext(bot, { message: task, cwd })
    return text
  }

  // bot_recall：搜自己的空间，开了开关时外加只读的全局。不搜原文层：普通会话的对话不给 Bot 看。
  async search(bot, { query, queries = [], since, until, includeHistory, order, level } = {}) {
    const spaceId = await this.space(bot.id)
    return this.memory.searchRelevant(query, {
      spaceIds: this.readableSpaces(bot, spaceId),
      limit: 8,
      queries,
      kinds: level === 'digests' ? ['digest', 'fact'] : ['fact'],
      since: since || null,
      until: until || null,
      ...(includeHistory === undefined ? {} : { includeHistory }),
      order: order || 'relevance',
    })
  }

  // 删 Bot：空间连同其中的记忆一起清掉。记忆用不了时也要把目录删干净。
  async removeBot(botId) {
    if (!BOT_ID_PATTERN.test(String(botId || ''))) return
    this.imports.delete(botId)
    try {
      await this.waitForMemory()
      if (this.memory?.db) this.memory.deleteSpace(botSpaceId(botId))
    } catch {
      console.warn('[bots] Could not delete the bot memory space.')
    }
    await rm(join(this.dataDir, 'bots', botId), { recursive: true, force: true }).catch(() => {})
  }
}
