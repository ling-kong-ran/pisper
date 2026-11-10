// Bot 群聊（room）的数据形状与校验：群、群里的消息、发消息的输入。外部输入一律按不可信 unknown 归一，
// 不合法时抛 PisperError。纯函数，不碰文件；落盘与索引在 room-service.mjs。
import { randomBytes } from 'node:crypto'
import { extname, isAbsolute } from 'node:path'
import { pisperError } from '../../../shared/errors.mjs'
import { ASSET_DOCUMENT_EXTENSIONS, mimeFromName, safeAttachmentName } from '../asset-content.mjs'

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
// 一条消息的附件：和会话输入框同一套上限（最多 8 个，单个 10 MB，合计 20 MB）。
export const MAX_ROOM_ATTACHMENTS = 8
export const MAX_ROOM_ATTACHMENT_BYTES = 10 * 1024 * 1024
export const MAX_ROOM_ATTACHMENTS_BYTES = 20 * 1024 * 1024
const MAX_ATTACHMENT_PATH_CHARS = 4_096
const BASE64_PATTERN = /^[A-Za-z0-9+/]*={0,2}$/
// 谁来回答：mention（默认，@ / 发给 / 回复谁就叫醒谁）或 host（没点名的话由主持人 Bot 判断该谁答）。
export const ROOM_ANSWER_MODES = ['mention', 'host']
// 主持人一次最多请几个 Bot 来回答（每个都要跑一整轮）。
export const ROOM_ROUTE_MAX_WAKES = 3

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

// 收件人（输入框的“发给”，没 @ 谁时才用）：'all' 表示所有成员 Bot；否则是 Bot id 列表。
// 列表为空只在主持人模式里有意义（交给主持人）；“@ 谁谁答”的群没有“只留言”，空的按所有人算，见 roomRecipients。
// “@ 谁谁答”的群里“发给”至少有人：空的（老的群、选的人都被移出了群、旧客户端发来的）按所有人算。
export function roomRecipients(recipients, answerMode) {
  return answerMode !== 'host' && Array.isArray(recipients) && recipients.length === 0
    ? 'all'
    : recipients
}

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
  // 主持人必须是成员。这次明确要主持人分派却没给成员当主持人：报错；主持人被移出群（删 Bot、改成员）
  // 或这次改的是别的字段：退回默认的“@ 谁谁答”，不让一次无关的修改失败。老的群没有这两个字段，也是默认。
  const hostBotId = String(merged.hostBotId ?? '').trim()
  const host = members.includes(hostBotId) ? hostBotId : ''
  if (value.answerMode === 'host' && !host) throw invalid('pick a member bot as the host')
  // 认不出的取值当没给：沿用旧值。
  const mode = ROOM_ANSWER_MODES.includes(value.answerMode) ? value.answerMode : current?.answerMode
  const answerMode = mode === 'host' && host ? 'host' : 'mention'
  // 没 @ 谁时默认发给所有人；主持人分派的群默认交给主持人（“发给”为空）。新建群、或这次切换了谁来回答
  // 而没一起给“发给”时按这个默认来；其余沿用记着的选择（老的群没有 answerMode，按“@ 谁谁答”算，不算切换）。
  const previousMode = current ? (current.answerMode === 'host' ? 'host' : 'mention') : null
  const recipients = Object.hasOwn(value, 'recipients')
    ? value.recipients
    : !current || previousMode !== answerMode
      ? answerMode === 'host'
        ? []
        : 'all'
      : current.recipients
  return {
    name,
    members,
    goal,
    cwd,
    // 维护简报的 Bot 必须是成员；移出群后自动清空。
    briefBotId: members.includes(briefBotId) ? briefBotId : '',
    recipients: roomRecipients(normalizeRecipients(recipients, members), answerMode),
    answerMode,
    hostBotId: host,
  }
}

function attachmentTooLarge(name) {
  return pisperError('too_large', 'attachment', `${name}: max 10 MB`)
}

/**
 * 一条消息带的附件：形状和会话输入框发的一样（图片、文字、文档带内容，桌面端选的文件只带本地路径）。
 * 大小按实际内容算，不信客户端报的 size。返回的条目仍带内容，归档成资产后消息里只留引用。
 * @param {unknown} value
 * @returns {{ kind: 'image' | 'text' | 'document' | 'path', name: string, mimeType: string, size: number, data?: string, text?: string, path?: string, extension?: string }[]}
 */
export function normalizeRoomAttachments(value) {
  if (value === undefined || value === null) return []
  if (!Array.isArray(value)) throw invalid('attachments must be a list')
  if (value.length > MAX_ROOM_ATTACHMENTS)
    throw pisperError('limit', 'attachment', `at most ${MAX_ROOM_ATTACHMENTS} per message`)
  const result = []
  let total = 0
  for (const item of value) {
    const input = record(item)
    const kind = String(input.kind ?? '')
    const name = safeAttachmentName(String(input.name ?? '').trim() || 'attachment')
    if (kind === 'path') {
      const path = String(input.path ?? '').trim()
      if (
        !path ||
        path.length > MAX_ATTACHMENT_PATH_CHARS ||
        path.includes('\0') ||
        !isAbsolute(path)
      )
        throw pisperError('invalid', 'path', `${name} is not an absolute local path`)
      result.push({ kind, name, mimeType: '', size: 0, path })
      continue
    }
    if (kind === 'text') {
      if (typeof input.text !== 'string') throw pisperError('invalid', 'attachment', name)
      const size = Buffer.byteLength(input.text, 'utf8')
      if (!size) throw pisperError('invalid', 'attachment', `${name} is empty`)
      if (size > MAX_ROOM_ATTACHMENT_BYTES) throw attachmentTooLarge(name)
      total += size
      result.push({
        kind,
        name,
        mimeType: String(input.mimeType || 'text/plain').slice(0, 120),
        size,
        text: input.text,
      })
      continue
    }
    if (kind !== 'image' && kind !== 'document')
      throw pisperError('unsupported', 'attachment', `${name}: choose an image, text or document`)
    const data = typeof input.data === 'string' ? input.data : ''
    // 先按长度粗判再跑正则：超大的字符串不值得逐字检查。
    if (!data || data.length > Math.ceil(MAX_ROOM_ATTACHMENT_BYTES / 3) * 4 + 4) {
      if (!data) throw pisperError('invalid', 'attachment', `${name} is empty`)
      throw attachmentTooLarge(name)
    }
    if (!BASE64_PATTERN.test(data)) throw pisperError('invalid', 'attachment', name)
    const size = Buffer.byteLength(data, 'base64')
    if (size > MAX_ROOM_ATTACHMENT_BYTES) throw attachmentTooLarge(name)
    total += size
    if (kind === 'image') {
      const given = String(input.mimeType || '')
      const mimeType = given.startsWith('image/') ? given.slice(0, 120) : mimeFromName(name)
      if (!mimeType.startsWith('image/'))
        throw pisperError('invalid', 'image', `${name} is not an image`)
      result.push({ kind, name, mimeType, size, data })
      continue
    }
    const extension = extname(name).toLowerCase()
    if (!ASSET_DOCUMENT_EXTENSIONS.has(extension))
      throw pisperError('unsupported', 'attachment', `${name}: choose an image, text or document`)
    result.push({
      kind,
      name,
      mimeType: String(input.mimeType || mimeFromName(name)).slice(0, 120),
      size,
      extension: extension.slice(1),
      data,
    })
  }
  if (total > MAX_ROOM_ATTACHMENTS_BYTES)
    throw pisperError('too_large', 'attachment', 'max 20 MB in total')
  return result
}

// 消息里存的附件引用：只有名字、类型、大小与资产 id（或本地路径），没有内容。读盘和投影时都按这个形状收。
export function roomAttachmentRefs(value) {
  if (!Array.isArray(value)) return []
  const refs = []
  for (const item of value.slice(0, MAX_ROOM_ATTACHMENTS)) {
    const input = record(item)
    const kind = String(input.kind ?? '')
    const name = safeAttachmentName(input.name)
    if (kind === 'path' && typeof input.path === 'string' && input.path)
      refs.push({ kind, name, path: input.path.slice(0, MAX_ATTACHMENT_PATH_CHARS) })
    else if (
      ['image', 'text', 'document'].includes(kind) &&
      typeof input.id === 'string' &&
      /^[A-Za-z0-9_-]{1,80}$/.test(input.id)
    )
      refs.push({
        id: input.id,
        kind,
        name,
        mimeType: String(input.mimeType || '').slice(0, 120),
        size: Number.isSafeInteger(input.size) && input.size > 0 ? input.size : 0,
      })
  }
  return refs
}

/**
 * 主持人的分派结果：从回复里取 JSON（第一个 { 到最后一个 }），bots 只留能叫醒的成员（去重、最多 max 个），
 * 写名字的也认；reason 是一行理由。解析不了、没有 bots 列表时为 null（这次分派失败）。
 * @param {unknown} raw
 * @param {{ id: string, name: string }[]} eligible
 * @param {number} [max]
 */
export function parseRouteReply(raw, eligible, max = ROOM_ROUTE_MAX_WAKES) {
  const source = String(raw || '')
  const start = source.indexOf('{')
  const end = source.lastIndexOf('}')
  if (start < 0 || end <= start) return null
  let value
  try {
    value = JSON.parse(source.slice(start, end + 1))
  } catch {
    return null
  }
  const input = record(value)
  if (!Array.isArray(input.bots)) return null
  const bots = []
  for (const item of input.bots) {
    const text = String(item ?? '').trim()
    const member =
      eligible.find((candidate) => candidate.id === text) ||
      eligible.find((candidate) => candidate.name && candidate.name === text.replace(/^@/, ''))
    if (member && !bots.includes(member.id)) bots.push(member.id)
    if (bots.length >= max) break
  }
  const reason = String(input.reason ?? '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, MAX_BRIEF_ITEM_CHARS)
  return { bots, reason }
}

/**
 * 你在群里发的一句话。mentions 由输入框的 @ 补全给出（Bot id）；正文里手打的 @名字 另由服务端解析补上。
 * 只带附件、不写字也可以（和会话输入框一样）。
 * @param {unknown} input
 * @param {string[]} members
 * @param {(id: string) => boolean} hasMessage
 */
export function normalizeUserRoomMessage(input, members, hasMessage) {
  const value = record(input)
  const text = String(value.text ?? '').trim()
  const attachments = normalizeRoomAttachments(value.attachments)
  if (!text && !attachments.length) throw pisperError('invalid', 'message', 'message is required')
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
    attachments,
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
