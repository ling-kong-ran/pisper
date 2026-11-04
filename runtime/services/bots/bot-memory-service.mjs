// 每个 Bot 自己的一份记忆：一条一条的短句（偏好、约定、事实、做完复盘的结论），存在
// <数据目录>/bots/<botId>/memory.json，原子写。不读写全局记忆；Bot 之间互不可见。
// 写入来源：对话的 remember / forget 决定、设置页手动增改删、每日整理。
import { randomBytes } from 'node:crypto'
import { rm } from 'node:fs/promises'
import { join } from 'node:path'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'

export const MAX_BOT_MEMORY_ENTRIES = 200
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

export class BotMemoryError extends Error {
  constructor(code, message, statusCode = 400) {
    super(message)
    this.code = code
    this.statusCode = statusCode
  }
}

function normalizeText(value) {
  const text = String(value ?? '')
    .replace(/\s+/g, ' ')
    .trim()
  if (!text) throw new BotMemoryError('bot_memory_empty', '记忆不能为空。')
  if (text.length > MAX_BOT_MEMORY_CHARS)
    throw new BotMemoryError('bot_memory_too_long', `一条记忆最多 ${MAX_BOT_MEMORY_CHARS} 个字符。`)
  return text
}

// 比较用的键：去掉空白与标点、统一大小写，“以后别 push 到 main。”与“以后别push到main”算同一条。
function sameKey(text) {
  return String(text || '')
    .normalize('NFKC')
    .toLowerCase()
    .replace(/[\s\p{P}]+/gu, '')
}

function restoredEntry(value) {
  if (!value || typeof value !== 'object') return null
  if (!ENTRY_ID_PATTERN.test(String(value.id || ''))) return null
  const text = String(value.text ?? '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, MAX_BOT_MEMORY_CHARS)
  if (!text) return null
  const source = BOT_MEMORY_SOURCES.includes(value.source) ? value.source : 'told'
  const createdAt =
    typeof value.createdAt === 'string' ? value.createdAt : new Date(0).toISOString()
  const updatedAt = typeof value.updatedAt === 'string' ? value.updatedAt : createdAt
  return { id: value.id, text, source, createdAt, updatedAt }
}

export class BotMemoryService {
  constructor({ dataDir, now = () => Date.now() }) {
    this.dataDir = dataDir
    this.now = now
    // 同一个 Bot 的读改写按顺序排队，避免并发写丢条目。
    this.chains = new Map()
  }

  file(botId) {
    if (!BOT_ID_PATTERN.test(String(botId || '')))
      throw new BotMemoryError('bot_not_found', 'Bot 不存在。', 404)
    return join(this.dataDir, 'bots', botId, 'memory.json')
  }

  async read(botId) {
    const state = await readJson(this.file(botId), null)
    const entries = (Array.isArray(state?.entries) ? state.entries : [])
      .map(restoredEntry)
      .filter(Boolean)
      .slice(-MAX_BOT_MEMORY_ENTRIES)
    const tidiedAt = typeof state?.tidiedAt === 'string' ? state.tidiedAt : null
    return { entries, tidiedAt }
  }

  update(botId, change) {
    const previous = this.chains.get(botId) || Promise.resolve()
    const next = previous
      .catch(() => {})
      .then(async () => {
        const state = await this.read(botId)
        const result = await change(state)
        await writeJsonAtomic(this.file(botId), {
          version: 1,
          tidiedAt: state.tidiedAt,
          ...result.state,
          entries: result.state.entries.slice(-MAX_BOT_MEMORY_ENTRIES),
        })
        return result.value
      })
    this.chains.set(botId, next)
    return next
  }

  async list(botId) {
    return (await this.read(botId)).entries
  }

  // 记住一条：和已有的某条说的是一回事时只更新它的时间，不重复记。
  async remember(botId, text, source = 'told') {
    const value = normalizeText(text)
    const kind = BOT_MEMORY_SOURCES.includes(source) ? source : 'told'
    return this.update(botId, (state) => {
      const at = new Date(this.now()).toISOString()
      const key = sameKey(value)
      const existing = state.entries.find((entry) => sameKey(entry.text) === key)
      if (existing) {
        const entries = state.entries.map((entry) =>
          entry === existing ? { ...entry, updatedAt: at } : entry,
        )
        return { state: { ...state, entries }, value: { ...existing, updatedAt: at } }
      }
      const entry = {
        id: `mem_${randomBytes(9).toString('base64url')}`,
        text: value,
        source: kind,
        createdAt: at,
        updatedAt: at,
      }
      return { state: { ...state, entries: [...state.entries, entry] }, value: entry }
    })
  }

  async edit(botId, id, text) {
    const value = normalizeText(text)
    return this.update(botId, (state) => {
      const found = state.entries.find((entry) => entry.id === id)
      if (!found) throw new BotMemoryError('bot_memory_not_found', '这条记忆不存在。', 404)
      const updated = { ...found, text: value, updatedAt: new Date(this.now()).toISOString() }
      return {
        state: {
          ...state,
          entries: state.entries.map((entry) => (entry.id === id ? updated : entry)),
        },
        value: updated,
      }
    })
  }

  // 忘掉：按 id，或者按内容（对话里说“忘掉 X”时）。找不到返回 null。
  forget(botId, { id = '', text = '' } = {}) {
    const key = text ? sameKey(text) : ''
    return this.update(botId, (state) => {
      const found = state.entries.find(
        (entry) => (id && entry.id === id) || (key && sameKey(entry.text) === key),
      )
      if (!found) return { state, value: null }
      return {
        state: { ...state, entries: state.entries.filter((entry) => entry !== found) },
        value: found,
      }
    })
  }

  // 每日整理：用整理后的清单替换（只能保留或改写已有条目、合并成更少的条，不能凭空多出来）。
  replaceAfterTidy(botId, kept) {
    return this.update(botId, (state) => {
      const at = new Date(this.now()).toISOString()
      const byId = new Map(state.entries.map((entry) => [entry.id, entry]))
      const entries = []
      for (const item of kept) {
        const original = byId.get(item.id)
        if (!original) continue
        byId.delete(item.id)
        const text = String(item.text ?? '')
          .replace(/\s+/g, ' ')
          .trim()
        entries.push(
          text && text !== original.text && text.length <= MAX_BOT_MEMORY_CHARS
            ? { ...original, text, source: 'tidy', updatedAt: at }
            : original,
        )
      }
      return { state: { ...state, entries, tidiedAt: at }, value: entries }
    })
  }

  markTidied(botId) {
    return this.update(botId, (state) => ({
      state: { ...state, tidiedAt: new Date(this.now()).toISOString() },
      value: null,
    }))
  }

  async removeBot(botId) {
    await rm(join(this.dataDir, 'bots', botId), { recursive: true, force: true }).catch(() => {})
  }
}

// 对话与子 agent 看到的那一段：最近更新的在后面，最多 60 条。
export function botMemoryContext(entries, limit = 60) {
  if (!entries?.length) return ''
  const shown = [...entries]
    .sort((left, right) => String(left.updatedAt).localeCompare(String(right.updatedAt)))
    .slice(-limit)
  return [
    '[你记得的事]',
    ...shown.map((entry) => `- (${entry.id.slice(4, 10)}) ${entry.text}`),
  ].join('\n')
}
