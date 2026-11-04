// Bot 之间的来往（C5）：一个 Bot 的对话决定给另一个 Bot 发话，话进对方的对话（标着来自谁），
// 对方的回复、它替你派的活的结果再回到发话的一方。一来一回算一跳，同一件事（同一条链）最多 6 跳；
// 同一对 Bot 每分钟有条数上限；“收到”“好的”这类客气话不发。权限不传递：对方用自己的设定干活。
// 这里是纯逻辑与内存状态（链只在内存里，和子 agent 一样，Runtime 重启后不恢复）。
import { randomBytes } from 'node:crypto'

export const MAX_BOT_LINK_HOPS = 6
export const MAX_BOT_LINK_MESSAGES_PER_MINUTE = 6
const MAX_LINKS = 200
const MAX_LINK_TEXT_CHARS = 300

// 消息里的来源标记：放在附件上下文里（转录不显示），运行时与转录投影据此认出这是别的 Bot 说的。
const TAG_PATTERN =
  /\[pisper:bot-message from=(bot_[A-Za-z0-9_-]{6,40}) chain=(ch_[A-Za-z0-9_-]{6,40}) hop=(\d{1,2}) reply=([01])\]/

export function newBotChainId() {
  return `ch_${randomBytes(9).toString('base64url')}`
}

export function botMessageTag({ from, chain, hop, reply }) {
  return `[pisper:bot-message from=${from} chain=${chain} hop=${hop} reply=${reply ? 1 : 0}]`
}

export function parseBotMessageTag(text) {
  const match = TAG_PATTERN.exec(String(text || ''))
  if (!match) return null
  return { from: match[1], chain: match[2], hop: Number(match[3]), reply: match[4] === '1' }
}

// 给对方模型看的说明：这句话是谁说的、要不要回。和标记一起放在附件上下文里。
export function botMessageContext({ fromName, chain, from, hop, reply }) {
  return [
    botMessageTag({ from, chain, hop, reply }),
    `这条消息来自 Bot「${fromName}」，不是用户。${
      reply
        ? '它在等你回话：你这一轮的回复会自动发回给它；要动手的照常派子 agent，做完的结果也会自动交给它。'
        : '这是它的回话，不用再回给它；需要它再做什么时，用 message 决定。'
    }`,
  ].join('\n')
}

// 客气话：不触发回话，免得两个 Bot 互相“好的”“谢谢”停不下来。
export function isCourtesyMessage(text) {
  const value = String(text || '').trim()
  if (!value || value.length > 16) return !value
  return /^(收到|好的?|好嘞|行|嗯+|明白|了解|知道了|没问题|谢谢|多谢|感谢|辛苦了|ok|okay|thanks?|thank you|got it|sure|👍|🙏)[。！!.,，~～\s]*(谢谢|thanks)?[。！!.~～\s]*$/iu.test(
    value,
  )
}

function clip(value) {
  const text = String(value || '')
    .replace(/\s+/g, ' ')
    .trim()
  return text.length > MAX_LINK_TEXT_CHARS ? `${text.slice(0, MAX_LINK_TEXT_CHARS)}…` : text
}

export class BotLinkService {
  constructor({ now = () => Date.now() } = {}) {
    this.now = now
    this.links = new Map()
    // 每对 Bot 最近一分钟发过的时间戳（不分方向）。
    this.recent = new Map()
  }

  pairKey(left, right) {
    return [left, right].sort().join('|')
  }

  // 发一句话之前先过这几道：不能发给自己、链上不超过 6 跳、这一对每分钟不超过上限。
  // 通过时返回 { ok: true, chain, hop }；不通过时 { ok: false, reason }。
  admit({ from, to, chain, hop = 1 }) {
    if (from === to) return { ok: false, reason: 'self' }
    if (hop > MAX_BOT_LINK_HOPS) return { ok: false, reason: 'hops', chain }
    const key = this.pairKey(from, to)
    const now = this.now()
    const recent = (this.recent.get(key) || []).filter((at) => now - at < 60_000)
    if (recent.length >= MAX_BOT_LINK_MESSAGES_PER_MINUTE) {
      this.recent.set(key, recent)
      return { ok: false, reason: 'rate', chain }
    }
    recent.push(now)
    this.recent.set(key, recent)
    return { ok: true, chain, hop }
  }

  // 发起方的一条链：时间线里一条“在等 B 回话”的事件，B 回了就合上。
  open({ chain, from, to, text }) {
    const existing = this.links.get(chain)
    const at = new Date(this.now()).toISOString()
    const link = existing
      ? { ...existing, status: 'waiting', updatedAt: at }
      : {
          id: chain,
          fromBotId: from,
          toBotId: to,
          text: clip(text),
          status: 'waiting',
          reason: '',
          hops: 1,
          openedAt: at,
          updatedAt: at,
        }
    this.links.set(chain, link)
    this.prune()
    return link
  }

  // 链上又走了一跳（回话或补话）；waiting = 发起方还在等结果。
  advance(chain, { hop, waiting }) {
    const link = this.links.get(chain)
    if (!link) return null
    const next = {
      ...link,
      hops: Math.max(link.hops, hop),
      status: waiting ? 'waiting' : 'answered',
      updatedAt: new Date(this.now()).toISOString(),
    }
    this.links.set(chain, next)
    return next
  }

  stop(chain, reason) {
    const link = this.links.get(chain)
    if (!link) return null
    const next = {
      ...link,
      status: 'stopped',
      reason,
      updatedAt: new Date(this.now()).toISOString(),
    }
    this.links.set(chain, next)
    return next
  }

  get(chain) {
    return this.links.get(chain) || null
  }

  // 某个 Bot 时间线上的来往：它发起的都列；别人找它的只在停下来时列（话本身已经在对话里）。
  listForBot(botId) {
    return [...this.links.values()]
      .filter(
        (link) => link.fromBotId === botId || (link.toBotId === botId && link.status === 'stopped'),
      )
      .map((link) => ({
        ...link,
        direction: link.fromBotId === botId ? 'out' : 'in',
        botId: link.fromBotId === botId ? link.toBotId : link.fromBotId,
      }))
  }

  removeBot(botId) {
    for (const [chain, link] of this.links)
      if (link.fromBotId === botId || link.toBotId === botId) this.links.delete(chain)
  }

  prune() {
    if (this.links.size <= MAX_LINKS) return
    const oldest = [...this.links.values()].sort((left, right) =>
      left.updatedAt.localeCompare(right.updatedAt),
    )
    for (const link of oldest.slice(0, this.links.size - MAX_LINKS)) this.links.delete(link.id)
  }
}
