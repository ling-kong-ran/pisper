// 群聊页的纯逻辑：消息列表 → 时间线条目（你的话、Bot 每一轮的回复、“这一轮”、交接行、系统行），
// 输入框上方的状态行、“找谁写一份简报”提示条，正文里的 @ 高亮切段，输入框里 @ 补全的查询。
// 不碰网络与 React，单测直接调。
import { assetDownloadUrl, assetMediaUrl } from '@/features/assets/public'
import type { ChatAttachment } from '@/types/chat'
import type {
  RoomApproval,
  RoomAttachment,
  RoomBrief,
  RoomMember,
  RoomMessage,
  RoomTodo,
} from './rooms-api'

export type TimelineMember = { id: string; name: string }

export type RoundEntry = { botId: string; state: 'replied' | 'skipped' | 'working' }

export type TimelineItem =
  | { kind: 'user'; message: RoomMessage; round: RoundEntry[] | null }
  | { kind: 'card'; message: RoomMessage; handoff: { from: string; hop: number } | null }
  | { kind: 'post'; message: RoomMessage }
  | { kind: 'system'; message: RoomMessage }
  | { kind: 'recalled'; message: RoomMessage }

// 这句话是不是叫醒了“所有人”（一轮“所有人”）：正文写了 @所有人 / @all / @everyone；或者没点名
// （没 @ 谁、没回复某个 Bot）、“发给”是所有人。点了名的话只叫点到的（和 Runtime 的 userWakeTargets 一致），
// 即使“发给”记着所有人也不算一轮——不然每句 @ 某人的话下面都会多一行“这一轮”。
export function addressesEveryone(
  message: RoomMessage,
  byId: ReadonlyMap<string, RoomMessage> = new Map(),
) {
  if (/(^|[^\p{L}\p{N}_])@(所有人|all|everyone)(?![\p{L}\p{N}_])/iu.test(message.text)) return true
  const repliedBot = message.replyTo ? byId.get(message.replyTo)?.author.kind === 'bot' : false
  return message.recipients === 'all' && !message.mentions?.length && !repliedBot
}

/**
 * 时间线：跳过的工作卡不单独占一行，只记进它回应的那句“所有人”的“这一轮”；
 * 别的 Bot 叫起来的工作卡前面画一行交接（谁 → 谁 · 第几跳）。
 */
export function timelineItems(messages: readonly RoomMessage[]): TimelineItem[] {
  const byId = new Map(messages.map((message) => [message.id, message]))
  const rounds = new Map<string, RoundEntry[]>()
  for (const message of messages) {
    const turn = message.turn
    if (!turn || message.author.kind !== 'bot') continue
    const botId = message.author.botId
    for (const trigger of [...turn.triggers, ...turn.alsoAnswered]) {
      const target = byId.get(trigger)
      if (!target || target.author.kind !== 'user' || !addressesEveryone(target, byId)) continue
      const entries = rounds.get(trigger) || []
      // 同一个 Bot 只记一次（插进来的几句话并成一轮时可能指向同一句）。
      if (entries.some((entry) => entry.botId === botId)) continue
      entries.push({
        botId,
        state:
          turn.state === 'skipped'
            ? 'skipped'
            : turn.state === 'running' || turn.state === 'waiting'
              ? 'working'
              : 'replied',
      })
      rounds.set(trigger, entries)
    }
  }
  const items: TimelineItem[] = []
  for (const message of messages) {
    if (message.recalled) {
      items.push({ kind: 'recalled', message })
      continue
    }
    if (message.author.kind === 'system') {
      items.push({ kind: 'system', message })
      continue
    }
    if (message.author.kind === 'user') {
      items.push({ kind: 'user', message, round: rounds.get(message.id) || null })
      continue
    }
    if (!message.turn) {
      items.push({ kind: 'post', message })
      continue
    }
    if (message.turn.state === 'skipped') continue
    const trigger = message.turn.triggers.map((id) => byId.get(id)).find(Boolean)
    items.push({
      kind: 'card',
      message,
      handoff:
        trigger?.author.kind === 'bot'
          ? { from: trigger.author.botId, hop: Math.max(2, message.chainHop) }
          : null,
    })
  }
  return items
}

export type TextSegment = { text: string; memberId?: string; everyone?: boolean }

// 正文切段：@成员名 与 @所有人 单独成段，其余原样。名字长的先匹配（“小克pro”不能被切成“小克”）。
export function mentionSegments(text: string, members: readonly TimelineMember[]): TextSegment[] {
  const named = members
    .filter((member) => member.name)
    .sort((left, right) => right.name.length - left.name.length)
  const segments: TextSegment[] = []
  let buffer = ''
  let index = 0
  while (index < text.length) {
    if (text[index] === '@') {
      const rest = text.slice(index + 1)
      const member = named.find((item) => {
        if (!rest.toLowerCase().startsWith(item.name.toLowerCase())) return false
        const next = rest.charAt(item.name.length)
        return !next || !/[\p{L}\p{N}_-]/u.test(next)
      })
      // 和 Runtime 的判定一致：@ 前面不能紧挨着字母数字（“a@all”不是在叫所有人）。
      const everyone = /[\p{L}\p{N}_]/u.test(text.charAt(index - 1))
        ? null
        : /^(所有人|all|everyone)(?![\p{L}\p{N}_])/iu.exec(rest)
      if (member || everyone) {
        if (buffer) segments.push({ text: buffer })
        buffer = ''
        const length = member ? member.name.length : everyone![0].length
        segments.push({
          text: text.slice(index, index + 1 + length),
          ...(member ? { memberId: member.id } : { everyone: true }),
        })
        index += 1 + length
        continue
      }
    }
    buffer += text[index]
    index += 1
  }
  if (buffer) segments.push({ text: buffer })
  return segments
}

// 输入框里光标前正在打的 @查询：返回 @ 的位置与已打的字；不在打 @ 时为 null。
export function activeMentionQuery(value: string, caret: number) {
  const before = value.slice(0, caret)
  const at = before.lastIndexOf('@')
  if (at < 0) return null
  if (at > 0 && /[\p{L}\p{N}_]/u.test(before[at - 1])) return null
  const query = before.slice(at + 1)
  if (/\s/.test(query) || query.length > 24) return null
  return { start: at, query }
}

// 发出去之前：正文里保留下来的 @成员 才算提及（补全选过、后来又删掉的不算）。
export function mentionedIds(text: string, members: readonly TimelineMember[]) {
  return [
    ...new Set(
      mentionSegments(text, members)
        .map((segment) => segment.memberId)
        .filter((id): id is string => Boolean(id)),
    ),
  ]
}

/**
 * 输入框里这句话要叫醒谁（和 Runtime 的 userWakeTargets 同一条规则）：点了名（@成员、@所有人、回复某个 Bot）
 * 就只叫点到的；没点名才按“发给”。chosen 是“发给”展开后的 id；返回去重的 id，暂停的由调用方再筛。
 */
export function composerWakeTargets(input: {
  text: string
  members: readonly TimelineMember[]
  chosen: readonly string[]
  repliedBot?: string
}) {
  const mentioned = mentionedIds(input.text, input.members)
  const mentionsAll = mentionSegments(input.text, input.members).some((segment) => segment.everyone)
  const addressed = mentioned.length > 0 || mentionsAll || Boolean(input.repliedBot)
  const targets = mentionsAll
    ? input.members.map((member) => member.id)
    : addressed
      ? [...mentioned, ...(input.repliedBot ? [input.repliedBot] : [])]
      : input.chosen
  return { addressed, mentionsAll, targets: [...new Set(targets)] }
}

// 用量：1234 → 1.2K，1_400_000 → 1.4M。
export function compactTokens(value: number) {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1).replace(/\.0$/, '')}M`
  if (value >= 1_000) return `${(value / 1_000).toFixed(1).replace(/\.0$/, '')}K`
  return String(Math.max(0, Math.round(value)))
}

// 简报编辑框里的一行待办：“[x] @小克 改确认框”，@me 是你。
export function todoLine(todo: RoomTodo, members: readonly RoomMember[]) {
  const owner =
    todo.owner === 'user'
      ? '@me '
      : todo.owner
        ? `@${members.find((member) => member.id === todo.owner)?.name || ''} `
        : ''
  return `${todo.done ? '[x] ' : ''}${owner}${todo.text}`
}

// 一行待办 → { 做完没有, 谁负责, 内容 }：“[x] @小克 改确认框”。
export function parseTodoLine(line: string, members: readonly RoomMember[]): RoomTodo | null {
  let rest = line.trim()
  if (!rest) return null
  let done = false
  const mark = /^\[(x|X| )\]\s*/.exec(rest)
  if (mark) {
    done = mark[1].toLowerCase() === 'x'
    rest = rest.slice(mark[0].length)
  }
  let owner = ''
  if (rest.startsWith('@')) {
    const named = members
      .filter((member) => member.name)
      .sort((left, right) => right.name.length - left.name.length)
    const member = named.find((item) => rest.slice(1).startsWith(item.name))
    if (member) {
      owner = member.id
      rest = rest.slice(member.name.length + 1).trim()
    } else if (/^@me\b/i.test(rest)) {
      owner = 'user'
      rest = rest.slice(3).trim()
    }
  }
  return rest ? { text: rest, owner, done } : null
}

// 消息的附件 → 会话里显示附件用的那种形状：图片走资产的内联地址（缩略图），文件带下载地址，
// 桌面端选的本地文件带路径（id 以 path: 开头，预览不去资产库取）。
export function roomAttachmentViews(attachments: readonly RoomAttachment[]): ChatAttachment[] {
  return attachments.map((attachment) => {
    if (attachment.kind === 'path')
      return {
        id: `path:${attachment.path}`,
        kind: 'path',
        name: attachment.name,
        path: attachment.path,
      }
    return {
      id: attachment.id,
      kind: attachment.kind,
      name: attachment.name,
      mimeType: attachment.mimeType,
      size: attachment.size,
      downloadUrl: assetDownloadUrl(attachment.id),
      ...(attachment.kind === 'image' ? { url: assetMediaUrl(attachment.id) } : {}),
    }
  })
}

export type TimelineKeyAction =
  { type: 'move'; index: number } | { type: 'reply' } | { type: 'recall' } | { type: 'process' }

type TimelineKey = {
  key: string
  altKey: boolean
  ctrlKey: boolean
  metaKey: boolean
}

// 这一条能不能回复、撤回 / 隐藏：和条目上的按钮一致（你的话；Bot 写完了的卡）。
function actionable(item: TimelineItem) {
  if (item.kind === 'user') return true
  if (item.kind !== 'card') return false
  const state = item.message.turn?.state
  return state !== 'running' && state !== 'waiting'
}

/**
 * 时间线的键盘操作（焦点在某一条消息上时）：上下方向键逐条移动，Home / End 到第一条 / 最后一条；
 * R 回复，Delete 撤回或隐藏（不接 Backspace：选中文字后习惯性按退格的人不该被确认框打断），
 * Enter 打开工作卡的过程。带 Ctrl / ⌘ / Alt 的组合键不接，
 * 留给浏览器与应用的快捷键。onMessage 为假时（焦点在条目里的按钮上）只接移动，不抢按钮自己的 Enter。
 */
export function timelineKeyAction(
  event: TimelineKey,
  item: TimelineItem,
  index: number,
  count: number,
  onMessage: boolean,
): TimelineKeyAction | null {
  if (event.altKey || event.ctrlKey || event.metaKey || count <= 0) return null
  const last = count - 1
  if (event.key === 'ArrowDown') return { type: 'move', index: Math.min(last, index + 1) }
  if (event.key === 'ArrowUp') return { type: 'move', index: Math.max(0, index - 1) }
  if (event.key === 'Home') return { type: 'move', index: 0 }
  if (event.key === 'End') return { type: 'move', index: last }
  if (!onMessage) return null
  if ((event.key === 'r' || event.key === 'R') && actionable(item)) return { type: 'reply' }
  if (event.key === 'Delete' && actionable(item)) return { type: 'recall' }
  if (event.key === 'Enter' && item.kind === 'card' && item.message.turn?.sessionId)
    return { type: 'process' }
  return null
}

// 还在做的一轮（在做、等你）：不在时间线里占一行，画成输入框上方的状态行；做完了回复才贴进时间线。
export function isLiveTurn(message: RoomMessage) {
  return message.turn?.state === 'running' || message.turn?.state === 'waiting'
}

export type RoomStatusEntry = {
  message: RoomMessage
  botId: string
  // 等你什么：批准、回答；null 是正在做。
  waiting: 'approval' | 'question' | null
  approval: RoomApproval | null
  // 这一轮已经结束，是它派出去的活在等你。
  pending: boolean
}

/**
 * 输入框上方的状态行：每个在做的一轮一行（在做 / 等你批准 / 等你回答）；一轮结束了、它派出去的活还在等你的，
 * 也占一行琥珀色的，批准就在这一行上。按时间先后排。
 */
export function roomStatusEntries(messages: readonly RoomMessage[]): RoomStatusEntry[] {
  const entries: RoomStatusEntry[] = []
  for (const message of messages) {
    const turn = message.turn
    if (!turn || message.author.kind !== 'bot' || message.recalled) continue
    if (turn.state === 'running' || turn.state === 'waiting') {
      entries.push({
        message,
        botId: message.author.botId,
        waiting: turn.state === 'waiting' ? turn.waiting || 'approval' : null,
        approval: turn.state === 'waiting' ? turn.approval : null,
        pending: false,
      })
    } else if (turn.pending) {
      entries.push({
        message,
        botId: message.author.botId,
        waiting: turn.pending.waiting,
        approval: turn.pending.approval,
        pending: true,
      })
    }
  }
  return entries
}

// 同一个人两分钟内接着说的话合并头部（头像、名字、时间只画一次），像 Slack。
export const ROOM_MERGE_WINDOW_MS = 2 * 60_000

export function mergesWithPrevious(previous: RoomMessage | undefined, message: RoomMessage) {
  if (!previous || previous.recalled || message.replyTo) return false
  const same =
    previous.author.kind === message.author.kind &&
    (message.author.kind !== 'bot' ||
      (previous.author.kind === 'bot' && previous.author.botId === message.author.botId))
  if (!same || message.author.kind === 'system') return false
  const gap = Date.parse(message.at) - Date.parse(previous.at)
  return Number.isFinite(gap) && gap >= 0 && gap <= ROOM_MERGE_WINDOW_MS
}

// 本地日期（年-月-日）：时间线在换天的地方画一条“今天 / 昨天 / 日期”分隔线。
export function roomDayKey(at: string) {
  const date = new Date(at)
  if (Number.isNaN(date.getTime())) return ''
  return `${date.getFullYear()}-${date.getMonth() + 1}-${date.getDate()}`
}

// 和 Runtime 的 ROOM_WAKE_MIN_CHARS 一致（runtime/services/rooms/room-model.mjs，测试核对）：
// 一次叫醒至少能带这么多字；简报之后没整理的消息超过它，就有 Bot 读不全了。
export const ROOM_BRIEF_PROMPT_CHARS = 24_000

/**
 * “找谁写一份简报”提示条：没有指定维护简报的 Bot（有的话 Runtime 读不下时会让它自动整理）、
 * 简报之后（以及“这段不用”之后）没整理的消息超过一次叫醒能带的量时出现。
 * 只按已经加载的消息算字数（更早的没加载时偏少，宁可晚一点出现）；条数用 lastSeq 算。
 */
export function briefPromptStats(input: {
  messages: readonly RoomMessage[]
  brief: RoomBrief | null
  lastSeq: number
  maintained: boolean
  skipUpTo?: number
}) {
  if (input.maintained) return null
  const from = Math.max(input.brief?.upToSeq || 0, input.skipUpTo || 0)
  const count = Math.max(0, input.lastSeq - from)
  const chars = input.messages
    .filter((message) => message.seq > from && !message.recalled)
    .reduce((sum, message) => sum + message.text.length, 0)
  return count > 0 && chars >= ROOM_BRIEF_PROMPT_CHARS ? { count, chars } : null
}
