// 群聊页的纯逻辑：消息列表 → 时间线条目（你的气泡、Bot 的工作卡、“这一轮”、交接行、系统行），
// 正文里的 @ 高亮切段，输入框里 @ 补全的查询。不碰网络与 React，单测直接调。
import type { RoomMember, RoomMessage, RoomTodo } from './rooms-api'

export type TimelineMember = { id: string; name: string }

export type RoundEntry = { botId: string; state: 'replied' | 'skipped' | 'working' }

export type TimelineItem =
  | { kind: 'user'; message: RoomMessage; round: RoundEntry[] | null }
  | { kind: 'card'; message: RoomMessage; handoff: { from: string; hop: number } | null }
  | { kind: 'post'; message: RoomMessage }
  | { kind: 'system'; message: RoomMessage }
  | { kind: 'recalled'; message: RoomMessage }

// 这句话是不是发给“所有人”：发给里选了所有人，或正文写了 @所有人 / @all / @everyone。
export function addressesEveryone(message: RoomMessage) {
  return (
    message.recipients === 'all' ||
    /(^|[^\p{L}\p{N}_])@(所有人|all|everyone)(?![\p{L}\p{N}_])/iu.test(message.text)
  )
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
      if (!target || target.author.kind !== 'user' || !addressesEveryone(target)) continue
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
