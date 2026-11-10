// 叫醒一个 Bot 时送什么：它上次读到之后的新消息（增量；席位会话自己记着前情），超出这一次能带的量时
// 用简报顶上、只送最后一段。还有叫醒谁的规则。纯函数；拼成文字在 runtime/prompts/rooms.mjs。
import { roomWakeContext } from '../../prompts/rooms.mjs'
import { ROOM_BOT_REPLY_CLIP_CHARS, ROOM_WAKE_MIN_CHARS, mentionsEveryone } from './room-model.mjs'

// 一个席位最多记多少张“越过去了、还没写完”的卡。
const MAX_LATE_CARDS = 20

// 一次叫醒能带多少字：这一轮模型上下文窗口的 10%（按一个 token 约 3 个字估），不少于 24 千字。
export function wakeBudgetChars(contextWindowTokens) {
  const window = Number(contextWindowTokens)
  const share = Number.isFinite(window) && window > 0 ? Math.floor(window * 0.1 * 3) : 0
  return Math.max(ROOM_WAKE_MIN_CHARS, share)
}

/**
 * 你发的一句话叫醒谁：@ 到的、“发给”里选的（所有人 = 全部成员）、你回复的那条消息的作者 Bot。
 * 暂停的 Bot 不叫，单独列出来（群里写一行说明）。
 * @param {{ mentions: string[], recipients: string[] | 'all', replyToAuthor?: string, text: string }} message
 * @param {{ id: string, enabled: boolean }[]} members
 */
export function userWakeTargets(message, members) {
  const everyone = message.recipients === 'all' || mentionsEveryone(message.text)
  const reasons = new Map()
  const add = (id, kind) => {
    if (!reasons.has(id)) reasons.set(id, kind)
  }
  for (const id of message.mentions) add(id, 'mention')
  if (message.replyToAuthor) add(message.replyToAuthor, 'reply')
  if (Array.isArray(message.recipients)) for (const id of message.recipients) add(id, 'recipient')
  if (everyone) for (const member of members) add(member.id, 'everyone')
  const byId = new Map(members.map((member) => [member.id, member]))
  const wake = []
  const paused = []
  for (const [id, kind] of reasons) {
    const member = byId.get(id)
    if (!member) continue
    if (member.enabled) wake.push({ botId: id, kind })
    else paused.push(id)
  }
  return { wake, paused }
}

/**
 * 主持人分派只管没点名的话：群设成 host、有主持人，而且这句话没有 @ 谁、没选“发给”、不是回复某个 Bot、
 * 没有 @所有人。点了名的（哪怕点的是暂停的 Bot）照常走上面的规则，不送给主持人。
 * @param {{ mentions: string[], recipients: string[] | 'all', replyToAuthor?: string, text: string }} message
 * @param {{ answerMode?: string, hostBotId?: string }} room
 */
export function routesToHost(message, room) {
  if (room.answerMode !== 'host' || !room.hostBotId) return false
  const addressed =
    message.mentions.length > 0 ||
    message.recipients === 'all' ||
    (Array.isArray(message.recipients) && message.recipients.length > 0) ||
    Boolean(message.replyToAuthor) ||
    mentionsEveryone(message.text)
  return !addressed
}

// Bot 的回复叫醒谁：只认正文里的 @（自动带的“回复”引用不叫人），不叫自己，不能叫“所有人”。
export function botWakeTargets({ authorId, mentions }, members) {
  const enabled = new Set(members.filter((member) => member.enabled).map((member) => member.id))
  return mentions.filter((id) => id !== authorId && enabled.has(id))
}

function clip(text, max, seq) {
  const value = String(text || '')
  return value.length > max ? `${value.slice(0, max)}… [full text in the room, #${seq}]` : value
}

function time(at) {
  const date = new Date(at)
  return Number.isNaN(date.getTime()) ? '' : date.toISOString().slice(11, 16)
}

/**
 * 这次送哪些消息：readSeq 之后的，略过还在进行中的工作卡（没有正文）与自己的消息。
 * 略过的进行中的卡记进 late：readSeq 会越过它，它写上结果后要在下一次叫醒时补送，不然那句回复谁也读不到。
 * 超过 budget 时从后往前取，能放下多少放多少；有简报且简报覆盖到被省掉的那段时用简报顶上。
 * @param {{
 *   messages: Record<string, any>[],
 *   readSeq: number,
 *   late?: number[],
 *   selfId: string,
 *   budget: number,
 *   names: (author: Record<string, any>) => string,
 * }} input
 */
export function selectUnread({ messages, readSeq, late = [], selfId, budget, names }) {
  const bySeq = new Map(messages.map((message) => [message.id, message.seq]))
  const owed = new Set(late)
  const pending = messages.filter(
    (message) =>
      (message.seq > readSeq || owed.has(message.seq)) &&
      !(message.author?.kind === 'bot' && message.author.botId === selfId),
  )
  const candidates = pending.filter(
    (message) => !(message.turn && ['running', 'skipped'].includes(message.turn.state)),
  )
  const stillRunning = pending
    .filter((message) => message.turn?.state === 'running')
    .map((message) => message.seq)
    .slice(-MAX_LATE_CARDS)
  const items = []
  let used = 0
  for (let index = candidates.length - 1; index >= 0; index -= 1) {
    const message = candidates[index]
    const text = message.recalled
      ? 'withdrew a message.'
      : message.author?.kind === 'bot'
        ? clip(message.text, ROOM_BOT_REPLY_CLIP_CHARS, message.seq)
        : String(message.text || '')
    // 附件只带名字（撤回的占位没有附件）。
    const files = message.recalled
      ? []
      : (Array.isArray(message.attachments) ? message.attachments : []).map((file) =>
          String(file?.name || 'attachment'),
        )
    const item = {
      seq: message.seq,
      time: time(message.at),
      author: names(message.author),
      text,
      ...(message.replyTo && bySeq.has(message.replyTo)
        ? { replyToSeq: bySeq.get(message.replyTo) }
        : {}),
      ...(files.length ? { attachments: files } : {}),
    }
    const size =
      item.text.length +
      item.author.length +
      24 +
      files.reduce((sum, file) => sum + file.length + 2, 0)
    if (items.length && used + size > budget) break
    // 单条就超过整份预算：截到预算以内，不至于一条也送不进去。
    if (size > budget) item.text = `${item.text.slice(0, Math.max(0, budget - 200))}…`
    items.unshift(item)
    used += size
  }
  return {
    items,
    omitted: candidates.length - items.length,
    upToSeq: messages.at(-1)?.seq || readSeq,
    late: stillRunning,
    chars: used,
  }
}

/**
 * 拼出叫醒时附带的上下文，以及转录里看得见的那一句（触发它的消息原文）。
 * @param {Record<string, any>} input
 */
export function buildWake({ room, bot, members, messages, seat, brief, reasons, budget, names }) {
  const selection = selectUnread({
    messages,
    readSeq: seat?.readSeq || 0,
    late: seat?.late || [],
    selfId: bot.id,
    budget,
    names,
  })
  // 省掉了前面一段、简报又没覆盖到被省掉的开头：只送最后一段，并告诉调用方该整理简报了。
  const firstSent = selection.items[0]?.seq || 0
  const needsBrief = selection.omitted > 0 && (!brief || (brief.upToSeq || 0) < firstSent - 1)
  const context = roomWakeContext({
    room,
    selfId: bot.id,
    members,
    brief: brief && brief.upToSeq ? brief : null,
    ownerName: (id) =>
      id === 'user' ? 'the user' : members.find((member) => member.id === id)?.name || id,
    items: selection.items,
    omitted: selection.omitted,
    recalled: seat?.recalls || [],
    reasons,
  })
  return {
    context,
    upToSeq: selection.upToSeq,
    late: selection.late,
    recalled: seat?.recalls || [],
    needsBrief,
    omitted: selection.omitted,
  }
}
