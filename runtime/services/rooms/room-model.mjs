// Bot 群聊（room）的数据形状与校验：群、群里的消息、发消息的输入。外部输入一律按不可信 unknown 归一，
// 不合法时抛 PisperError。纯函数，不碰文件；落盘与索引在 room-service.mjs。
import { randomBytes } from 'node:crypto'
import { pisperError } from '../../../shared/errors.mjs'

export const MAX_ROOMS = 32
export const MAX_ROOM_MEMBERS = 8
export const MAX_ROOM_NAME_CHARS = 40
export const MAX_ROOM_GOAL_CHARS = 200
export const MAX_ROOM_MESSAGE_CHARS = 12_000
// 群里一条消息最多存这么多字；Bot 的长回复超出部分不进群记录（完整的在它的席位会话里）。
export const MAX_ROOM_STORED_CHARS = 20_000
// 一条链（从你的一句话算起）总共最多叫醒几次：6 跳只限深度，挡不住一个 Bot @ 好几个的分叉。
export const ROOM_CHAIN_MAX_WAKES = 12
// 一次叫醒最少能带多少字（约 8k token）；实际按这一轮模型上下文窗口的 10% 算，不低于它。
export const ROOM_WAKE_MIN_CHARS = 24_000
// 别的 Bot 的长回复送进上下文时截到这么长，后面写“全文在群里第 N 条”。
export const ROOM_BOT_REPLY_CLIP_CHARS = 4_000
// 简报里决定、待办各最多几条，每条多长。
export const MAX_BRIEF_ITEMS = 12
export const MAX_BRIEF_ITEM_CHARS = 200

export const ROOM_ID_PATTERN = /^room_[A-Za-z0-9_-]{6,40}$/
export const ROOM_MESSAGE_ID_PATTERN = /^msg_[A-Za-z0-9_-]{6,40}$/
const BOT_ID_PATTERN = /^bot_[A-Za-z0-9_-]{6,40}$/

export function newRoomId() {
  return `room_${randomBytes(9).toString('base64url')}`
}

export function newRoomMessageId() {
  return `msg_${randomBytes(9).toString('base64url')}`
}

function record(value) {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : {}
}

function invalid(detail) {
  return pisperError('invalid', 'input', detail)
}

function botIds(value, max) {
  if (value === undefined) return undefined
  if (!Array.isArray(value)) throw invalid('members must be a list of bot ids')
  const ids = []
  for (const item of value) {
    const id = String(item ?? '').trim()
    if (!BOT_ID_PATTERN.test(id)) throw invalid('unknown member')
    if (!ids.includes(id)) ids.push(id)
  }
  if (ids.length > max) throw pisperError('limit', 'bot', `a room holds at most ${max} bots`)
  return ids
}

// 收件人：'all' 表示所有成员 Bot；否则是 Bot id 列表（可以为空，只发到群里不叫醒谁）。
export function normalizeRecipients(value, members) {
  if (value === 'all') return 'all'
  if (value === undefined || value === null) return []
  const ids = botIds(value, MAX_ROOM_MEMBERS) || []
  return ids.filter((id) => members.includes(id))
}

/**
 * 群的设定：名字、成员、可选的项目目录、目标、维护简报的 Bot、输入框里记住的收件人。
 * current 存在时是修改：没给的字段沿用旧值。
 * @param {unknown} input
 * @param {Record<string, any> | null} [current]
 */
export function normalizeRoomInput(input, current = null) {
  const value = record(input)
  const merged = { ...(current || {}), ...value }
  const name = String(merged.name ?? '').trim()
  if (!name) throw invalid('give the room a name')
  if (name.length > MAX_ROOM_NAME_CHARS)
    throw pisperError('too_large', 'input', `room name max ${MAX_ROOM_NAME_CHARS} characters`)
  const members = botIds(merged.members ?? [], MAX_ROOM_MEMBERS) || []
  if (!members.length) throw invalid('pick at least one bot')
  const goal = String(merged.goal ?? '').trim()
  if (goal.length > MAX_ROOM_GOAL_CHARS)
    throw pisperError('too_large', 'input', `goal max ${MAX_ROOM_GOAL_CHARS} characters`)
  const cwd = String(merged.cwd ?? '').trim()
  if (cwd.length > 4_096) throw pisperError('too_large', 'folder')
  const briefBotId = String(merged.briefBotId ?? '').trim()
  return {
    name,
    members,
    goal,
    cwd,
    // 维护简报的 Bot 必须是成员；移出群后自动清空。
    briefBotId: members.includes(briefBotId) ? briefBotId : '',
    recipients: normalizeRecipients(merged.recipients, members),
  }
}

/**
 * 你在群里发的一句话。mentions 由输入框的 @ 补全给出（Bot id）；正文里手打的 @名字 另由服务端解析补上。
 * @param {unknown} input
 * @param {string[]} members
 * @param {(id: string) => boolean} hasMessage
 */
export function normalizeUserRoomMessage(input, members, hasMessage) {
  const value = record(input)
  const text = String(value.text ?? '').trim()
  if (!text) throw pisperError('invalid', 'message', 'message is required')
  if (text.length > MAX_ROOM_MESSAGE_CHARS)
    throw pisperError('too_large', 'message', `max ${MAX_ROOM_MESSAGE_CHARS} characters`)
  const mentions = (botIds(value.mentions ?? [], MAX_ROOM_MEMBERS) || []).filter((id) =>
    members.includes(id),
  )
  const replyTo = String(value.replyTo ?? '').trim()
  if (replyTo && (!ROOM_MESSAGE_ID_PATTERN.test(replyTo) || !hasMessage(replyTo)))
    throw pisperError('not_found', 'message', 'the message you replied to is gone')
  return {
    text,
    mentions,
    ...(replyTo ? { replyTo } : {}),
    recipients: normalizeRecipients(value.recipients, members),
  }
}

// 正文里的 @名字 → Bot id。名字长的先匹配（“小克pro”不能被当成“小克”），名字后面必须不是字母数字，
// 避免 @小克proxy 误中小克pro。返回按出现顺序去重的 id。
export function parseMentions(text, members) {
  const source = String(text || '')
  const named = members
    .filter((member) => member?.name)
    .toSorted((left, right) => right.name.length - left.name.length)
  const found = []
  for (let index = source.indexOf('@'); index >= 0; index = source.indexOf('@', index + 1)) {
    const rest = source.slice(index + 1)
    const member = named.find((item) => {
      if (!rest.toLowerCase().startsWith(item.name.toLowerCase())) return false
      const next = rest.charAt(item.name.length)
      return !next || !/[\p{L}\p{N}_-]/u.test(next)
    })
    if (member && !found.includes(member.id)) found.push(member.id)
  }
  return found
}

// “所有人”的写法：@所有人 / @all / @everyone。
export function mentionsEveryone(text) {
  return /(^|[^\p{L}\p{N}_])@(所有人|all|everyone)(?![\p{L}\p{N}_])/iu.test(String(text || ''))
}

// 被 @所有人 叫醒、觉得和自己无关时的回复：只认这一个写法（大小写、两边空白与方括号可有可无）。
export function isPassReply(text) {
  return /^\[?\s*pass\s*\]?[.!。！]?$/i.test(String(text || '').trim())
}

function briefItems(value, map) {
  return (Array.isArray(value) ? value : []).map(map).filter(Boolean).slice(0, MAX_BRIEF_ITEMS)
}

function briefText(value) {
  const text = String(value ?? '')
    .replace(/\s+/g, ' ')
    .trim()
  return text ? text.slice(0, MAX_BRIEF_ITEM_CHARS) : ''
}

/**
 * 简报：已定的事、待办（谁负责、做完没有）。目标是群的设定（room.goal），不在简报里另存一份。
 * Bot 整理出来的 JSON 与你手改的都走这里归一。
 * owner 只认成员 Bot 的 id 或 'user'，别的丢掉。
 * @param {unknown} input
 * @param {string[]} members
 */
export function normalizeBrief(input, members) {
  const value = record(input)
  return {
    decisions: briefItems(value.decisions, (item) => briefText(item) || null),
    todos: briefItems(value.todos, (item) => {
      const todo = record(item)
      const text = briefText(todo.text)
      if (!text) return null
      const owner = String(todo.owner ?? '')
      return {
        text,
        owner: owner === 'user' || members.includes(owner) ? owner : '',
        done: todo.done === true,
      }
    }),
  }
}

// 从 Bot 的回复里取简报 JSON：第一个 { 到最后一个 }；解析不了时为 null（这次整理当没做）。
export function parseBriefReply(raw, members) {
  const source = String(raw || '')
  const start = source.indexOf('{')
  const end = source.lastIndexOf('}')
  if (start < 0 || end <= start) return null
  try {
    return normalizeBrief(JSON.parse(source.slice(start, end + 1)), members)
  } catch {
    return null
  }
}
