// 渠道消息分派给 Bot（B3）：纯函数，可独立测试。设计见 README「入口与分派」。
// 没点名任何 Bot、也不是某个 Bot 的默认聊天时返回 none，渠道照旧直接和 Pisper 对话（开关关闭时同样不调用）。
// 点名方式：/名字 开头；@名字；或 @Pisper（或其他 @ 提及）后紧跟名字。名字不区分大小写，取最长匹配。
// 私聊里也可以直接以“名字，”“名字：”或“名字 ”开头。

const SEPARATOR = /^[\s,，:：、]/

function lower(value) {
  return String(value ?? '').toLocaleLowerCase()
}

// text 以某个 Bot 的名字开头、且名字后是分隔符或结尾：返回最长的那个。
function leadingBot(text, bots) {
  const source = lower(text)
  let best = null
  for (const bot of bots) {
    const name = lower(bot.name)
    if (!name || !source.startsWith(name)) continue
    const rest = text.slice(bot.name.length)
    if (rest && !SEPARATOR.test(rest)) continue
    if (!best || bot.name.length > best.name.length) best = bot
  }
  return best
}

function rest(text, bot) {
  return text
    .slice(bot.name.length)
    .replace(/^[\s,，:：、]+/, '')
    .trim()
}

// 拆出消息里的点名：{ bot, text, mentioned }。mentioned 表示开头有 @ 提及（叫的是 Pisper 或某个 Bot）。
export function parseBotMention(content, bots) {
  let text = String(content ?? '').trim()
  let mentioned = false
  if (text.startsWith('/')) {
    const bot = leadingBot(text.slice(1), bots)
    if (bot) return { bot, text: rest(text.slice(1), bot), mentioned: true, command: true }
    return { bot: null, text, mentioned: false, command: true }
  }
  for (let guard = 0; guard < 8 && text.startsWith('@'); guard += 1) {
    mentioned = true
    const bot = leadingBot(text.slice(1), bots)
    if (bot) return { bot, text: rest(text.slice(1), bot), mentioned, command: false }
    // 不是 Bot 的 @（如 @Pisper、平台占位 @_user_1）：跳过这个词，看后面。
    text = text.replace(/^@\S+\s*/, '')
  }
  if (mentioned) {
    const bot = leadingBot(text, bots)
    if (bot) return { bot, text: rest(text, bot), mentioned, command: false }
  }
  return { bot: null, text, mentioned, command: false }
}

// 这个 Bot 在这个聊天里、对这个人可用吗？返回 null 表示可以，否则是拒绝原因。
// 群聊必须写明谁能叫它（模型层已强制）；私聊没写名单时只认渠道的创建者，
// 渠道的“所有人可用”不放宽 Bot 的名单。
export function botRefusal(bot, { platform, peerId, chatType, senderId }, { ownerId = '' } = {}) {
  if (!bot.enabled) return 'paused'
  const kind = chatType === 'p2p' ? 'p2p' : 'group'
  const rules = (bot.channels || []).filter(
    (channel) =>
      channel.platform === platform &&
      channel.chatType === kind &&
      (channel.peerIds === null || channel.peerIds.includes(String(peerId))),
  )
  if (!rules.length) return 'not_in_chat'
  const allowed = rules.some((channel) =>
    channel.senderIds.length
      ? channel.senderIds.includes(String(senderId))
      : kind === 'p2p' && Boolean(ownerId) && String(senderId) === String(ownerId),
  )
  return allowed ? null : 'sender_not_allowed'
}

// 分派：{ kind: 'bot', bot, text } | { kind: 'refused', bot, reason } | { kind: 'none' }。
export function routeChannelMessage(message, bots, { ownerId = '' } = {}) {
  const parsed = parseBotMention(message.content, bots)
  let bot = parsed.bot
  let text = parsed.text
  // 私聊里直接以名字开头（名字后要有分隔符）也算点名；群里必须 @ 或 /，免得闲聊里提到名字就派活。
  if (!bot && !parsed.command && message.chatType === 'p2p') {
    const named = leadingBot(parsed.text, bots)
    if (named) {
      bot = named
      text = rest(parsed.text, named)
    }
  }
  // 默认 Bot：没点名，但这个聊天是某个 Bot 的默认聊天，且是私聊或 @ 了 Pisper。
  if (!bot && !parsed.command && (message.chatType === 'p2p' || parsed.mentioned)) {
    const key = `${message.platform}:${message.peerId}`
    bot = bots.find((item) => (item.defaultIn || []).includes(key)) || null
  }
  if (!bot) return { kind: 'none' }
  const reason = botRefusal(bot, message, { ownerId })
  if (reason) return { kind: 'refused', bot, reason }
  return { kind: 'bot', bot, text }
}

// 这个聊天里这个人能叫的 Bot（/bots 用）。
export function botsForChat(message, bots, { ownerId = '' } = {}) {
  return bots.filter((bot) => !botRefusal(bot, message, { ownerId }))
}

export function refusalText(bot, reason) {
  if (reason === 'paused') return `${bot.name} 已暂停，不接新任务。`
  if (reason === 'sender_not_allowed') return `你不在 ${bot.name} 的名单里，叫不动它。`
  return `${bot.name} 不在这个聊天里。`
}
