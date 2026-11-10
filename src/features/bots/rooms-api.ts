// Bot 群聊接口：/api/rooms。群里是一份共用的聊天记录：你的话、Bot 每一轮的工作卡（在做 → 做完的结果）、
// 系统行（链停了、Bot 暂停了没叫醒）。响应按不可信数据校验：关键字段不对整体拒绝，可选字段给安全默认值。
import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'
import type { ChatAttachment } from '@/types/chat'
import { isBotCharacter, isBotColor, type BotAvatarValue } from './avatar'

export type RoomActivity = { working: number; needsYou: boolean }

export type Room = {
  id: string
  name: string
  members: string[]
  goal: string
  cwd: string
  // 维护简报的 Bot；空表示没人维护（只能手动整理或自己写）。
  briefBotId: string
  // 输入框“发给”记住的收件人：'all' 是所有成员。
  recipients: string[] | 'all'
  activity: RoomActivity
}

export type RoomApproval = {
  id: string
  toolName: string
  argsPreview: string
  severity: 'standard' | 'critical'
  reason: string
}

export type RoomTurnState = 'running' | 'waiting' | 'done' | 'error' | 'stopped' | 'skipped'

// 一张工作卡 = 某个 Bot 的一轮：在做时带当前这一步与最近几步，做完了带改动统计。
export type RoomTurn = {
  sessionId: string
  state: RoomTurnState
  waiting: 'approval' | 'question' | null
  reason: 'wake' | 'note' | 'report'
  triggers: string[]
  alsoAnswered: string[]
  current: string
  steps: { name: string; summary: string }[]
  stepCount: number
  changes: { files: number; added: number | null; removed: number | null } | null
  error: string
  approval: RoomApproval | null
  // 这一轮已经结束、它派出去的活还在等你：批准或回答。
  pending: { waiting: 'approval' | 'question'; approval: RoomApproval | null } | null
  startedAt: string
  endedAt: string
}

// 消息带的附件：只有引用（资产 id 或桌面端选的本地路径），内容在资产库里。
export type RoomAttachment = {
  id: string
  kind: 'image' | 'text' | 'document' | 'path'
  name: string
  mimeType: string
  size: number
  path: string
}

export type RoomAuthor = { kind: 'user' } | { kind: 'bot'; botId: string } | { kind: 'system' }

export type RoomEvent = {
  type: string
  botId: string
  reason: string
  from: string
  to: string
}

export type RoomMessage = {
  seq: number
  id: string
  at: string
  author: RoomAuthor
  text: string
  attachments: RoomAttachment[]
  mentions: string[]
  replyTo: string
  recipients: string[] | 'all' | null
  turn: RoomTurn | null
  // 链上的第几跳：你的话是 1，Bot 叫 Bot 依次加一。
  chainHop: number
  event: RoomEvent | null
  recalled: { seenBy: string[] } | null
}

export type RoomMemberStatus = {
  state: 'idle' | 'running' | 'waiting'
  waiting: 'approval' | 'question' | null
  current: string
}

export type RoomMember = {
  id: string
  name: string
  avatar: BotAvatarValue
  enabled: boolean
  deleted: boolean
  sessionId: string
  readSeq: number
  unread: number
  status: RoomMemberStatus
}

export type RoomTodo = { text: string; owner: string; done: boolean }
export type RoomBrief = {
  decisions: string[]
  todos: RoomTodo[]
  upToSeq: number
  by: string
  at: string
}

export type RoomDetail = {
  room: Room
  members: RoomMember[]
  brief: RoomBrief | null
  lastSeq: number
  activity: RoomActivity
  totalTokens: number
  briefing: boolean
}

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {}
}

function string(value: unknown, fallback = '') {
  return typeof value === 'string' ? value : fallback
}

function strings(value: unknown) {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === 'string')
    : []
}

function count(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

function nullableCount(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function recipients(value: unknown): string[] | 'all' {
  return value === 'all' ? 'all' : strings(value)
}

function parseActivity(value: unknown): RoomActivity {
  const input = object(value)
  return { working: count(input.working), needsYou: input.needsYou === true }
}

export function parseRoom(value: unknown): Room {
  const input = object(value)
  if (typeof input.id !== 'string' || typeof input.name !== 'string') throw invalidResponseError()
  return {
    id: input.id,
    name: input.name,
    members: strings(input.members),
    goal: string(input.goal),
    cwd: string(input.cwd),
    briefBotId: string(input.briefBotId),
    recipients: recipients(input.recipients),
    activity: parseActivity(input.activity),
  }
}

const TURN_STATES: readonly RoomTurnState[] = [
  'running',
  'waiting',
  'done',
  'error',
  'stopped',
  'skipped',
]

function parseApproval(value: unknown): RoomApproval | null {
  const input = object(value)
  if (typeof input.id !== 'string' || !input.id) return null
  return {
    id: input.id,
    toolName: string(input.toolName),
    argsPreview: string(input.argsPreview),
    severity: input.severity === 'critical' ? 'critical' : 'standard',
    reason: string(input.reason),
  }
}

function parsePending(value: unknown): RoomTurn['pending'] {
  const input = object(value)
  if (input.waiting !== 'approval' && input.waiting !== 'question') return null
  return { waiting: input.waiting, approval: parseApproval(input.approval) }
}

function parseTurn(value: unknown): RoomTurn | null {
  if (!value || typeof value !== 'object') return null
  const input = object(value)
  const state = TURN_STATES.find((item) => item === input.state) ?? 'done'
  const changes = object(input.changes)
  return {
    sessionId: string(input.sessionId),
    state,
    waiting: input.waiting === 'approval' || input.waiting === 'question' ? input.waiting : null,
    reason: input.reason === 'note' || input.reason === 'report' ? input.reason : 'wake',
    triggers: strings(input.triggers),
    alsoAnswered: strings(input.alsoAnswered),
    current: string(input.current),
    steps: (Array.isArray(input.steps) ? input.steps : []).map((step) => {
      const item = object(step)
      return { name: string(item.name), summary: string(item.summary) }
    }),
    stepCount: count(input.stepCount),
    changes:
      typeof changes.files === 'number' && changes.files > 0
        ? {
            files: changes.files,
            added: nullableCount(changes.added),
            removed: nullableCount(changes.removed),
          }
        : null,
    error: string(input.error),
    approval: parseApproval(input.approval),
    pending: parsePending(input.pending),
    startedAt: string(input.startedAt),
    endedAt: string(input.endedAt),
  }
}

const ATTACHMENT_KINDS: readonly RoomAttachment['kind'][] = ['image', 'text', 'document', 'path']

function parseAttachments(value: unknown): RoomAttachment[] {
  if (!Array.isArray(value)) return []
  return value.flatMap((item) => {
    const input = object(item)
    const kind = ATTACHMENT_KINDS.find((entry) => entry === input.kind)
    if (!kind) return []
    const path = string(input.path)
    const id = kind === 'path' ? '' : string(input.id)
    if (kind === 'path' ? !path : !id) return []
    return [
      {
        id,
        kind,
        name: string(input.name, 'attachment'),
        mimeType: string(input.mimeType),
        size: count(input.size),
        path,
      },
    ]
  })
}

function parseAuthor(value: unknown): RoomAuthor {
  const input = object(value)
  if (input.kind === 'bot' && typeof input.botId === 'string')
    return { kind: 'bot', botId: input.botId }
  if (input.kind === 'system') return { kind: 'system' }
  return { kind: 'user' }
}

export function parseRoomMessage(value: unknown): RoomMessage {
  const input = object(value)
  if (typeof input.id !== 'string' || typeof input.seq !== 'number') throw invalidResponseError()
  const event = input.event ? object(input.event) : null
  const recalled = input.recalled ? object(input.recalled) : null
  return {
    seq: input.seq,
    id: input.id,
    at: string(input.at),
    author: parseAuthor(input.author),
    text: string(input.text),
    attachments: parseAttachments(input.attachments),
    mentions: strings(input.mentions),
    replyTo: string(input.replyTo),
    recipients: input.recipients === undefined ? null : recipients(input.recipients),
    turn: parseTurn(input.turn),
    chainHop: count(object(input.chain).hop),
    event: event
      ? {
          type: string(event.type),
          botId: string(event.botId),
          reason: string(event.reason),
          from: string(event.from),
          to: string(event.to),
        }
      : null,
    recalled: recalled ? { seenBy: strings(recalled.seenBy) } : null,
  }
}

function parseAvatar(value: unknown): BotAvatarValue {
  const input = object(value)
  return {
    character: isBotCharacter(input.character) ? input.character : 'blob',
    color: isBotColor(input.color) ? input.color : 'blue',
  }
}

function parseMember(value: unknown): RoomMember {
  const input = object(value)
  if (typeof input.id !== 'string') throw invalidResponseError()
  const status = object(input.status)
  return {
    id: input.id,
    name: string(input.name),
    avatar: parseAvatar(input.avatar),
    enabled: input.enabled !== false,
    deleted: input.deleted === true,
    sessionId: string(input.sessionId),
    readSeq: count(input.readSeq),
    unread: count(input.unread),
    status: {
      state: status.state === 'running' || status.state === 'waiting' ? status.state : 'idle',
      waiting:
        status.waiting === 'approval' || status.waiting === 'question' ? status.waiting : null,
      current: string(status.current),
    },
  }
}

export function parseBrief(value: unknown): RoomBrief | null {
  if (!value || typeof value !== 'object') return null
  const input = object(value)
  return {
    decisions: strings(input.decisions),
    todos: (Array.isArray(input.todos) ? input.todos : []).map((todo) => {
      const item = object(todo)
      return { text: string(item.text), owner: string(item.owner), done: item.done === true }
    }),
    upToSeq: count(input.upToSeq),
    by: string(input.by),
    at: string(input.at),
  }
}

export function fetchRooms(signal?: AbortSignal) {
  return requestJson('/api/rooms', {
    signal,
    parse: (value) => {
      const rooms = object(value).rooms
      if (!Array.isArray(rooms)) throw invalidResponseError()
      return rooms.map(parseRoom)
    },
  })
}

export type RoomDraft = { name: string; members: string[]; goal?: string; cwd?: string }

export function createRoom(draft: RoomDraft) {
  return requestJson('/api/rooms', {
    method: 'POST',
    data: draft,
    parse: (value) => parseRoom(object(value).room),
  })
}

export function updateRoom(
  id: string,
  patch: Partial<Pick<Room, 'name' | 'members' | 'goal' | 'cwd' | 'briefBotId' | 'recipients'>>,
) {
  return requestJson(`/api/rooms/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    data: patch,
    parse: (value) => parseRoom(object(value).room),
  })
}

export function deleteRoom(id: string) {
  return requestJson(`/api/rooms/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

export function fetchRoom(id: string, signal?: AbortSignal): Promise<RoomDetail> {
  return requestJson(`/api/rooms/${encodeURIComponent(id)}`, {
    signal,
    parse: (value) => {
      const input = object(value)
      if (!Array.isArray(input.members)) throw invalidResponseError()
      return {
        room: parseRoom(input.room),
        members: input.members.map(parseMember),
        brief: parseBrief(input.brief),
        lastSeq: count(input.lastSeq),
        activity: parseActivity(input.activity),
        totalTokens: count(object(input.usage).totalTokens),
        briefing: input.briefing === true,
      }
    },
  })
}

export type RoomMessagePage = { messages: RoomMessage[]; lastSeq: number; hasMore: boolean }

export function fetchRoomMessages(
  id: string,
  query: { before?: number; after?: number; watch?: readonly string[] } = {},
  signal?: AbortSignal,
): Promise<RoomMessagePage> {
  const params = new URLSearchParams()
  if (query.before) params.set('before', String(query.before))
  if (query.after !== undefined) params.set('after', String(query.after))
  if (query.watch?.length) params.set('watch', query.watch.slice(0, 50).join(','))
  const search = params.size ? `?${params}` : ''
  return requestJson(`/api/rooms/${encodeURIComponent(id)}/messages${search}`, {
    signal,
    parse: (value) => {
      const input = object(value)
      if (!Array.isArray(input.messages)) throw invalidResponseError()
      return {
        messages: input.messages.map(parseRoomMessage),
        lastSeq: count(input.lastSeq),
        hasMore: input.hasMore === true,
      }
    },
  })
}

export type RoomMessageDraft = {
  text: string
  // 和会话输入框发的同一种附件（图片、文字、文档带内容，桌面端选的文件只带路径）。
  attachments?: ChatAttachment[]
  mentions: string[]
  recipients: string[] | 'all'
  replyTo?: string
}

export function postRoomMessage(id: string, draft: RoomMessageDraft) {
  return requestJson(`/api/rooms/${encodeURIComponent(id)}/messages`, {
    method: 'POST',
    data: draft,
    parse: (value) => ({
      message: parseRoomMessage(object(value).message),
      woke: strings(object(value).woke),
    }),
  })
}

export function recallRoomMessage(id: string, messageId: string) {
  return requestJson(
    `/api/rooms/${encodeURIComponent(id)}/messages/${encodeURIComponent(messageId)}/recall`,
    {
      method: 'POST',
      data: {},
      parse: (value) => ({
        seenBy: strings(object(value).seenBy),
        briefMayContain: object(value).briefMayContain === true,
      }),
    },
  )
}

export function resolveRoomApproval(
  id: string,
  messageId: string,
  approvalId: string,
  approve: boolean,
) {
  return requestJson(
    `/api/rooms/${encodeURIComponent(id)}/messages/${encodeURIComponent(messageId)}/approval`,
    { method: 'POST', data: { approvalId, approve } },
  )
}

export function stopRoomTurn(id: string, messageId: string) {
  return requestJson(
    `/api/rooms/${encodeURIComponent(id)}/messages/${encodeURIComponent(messageId)}/stop`,
    { method: 'POST', data: {} },
  )
}

export function refreshRoomBrief(id: string, botId?: string) {
  return requestJson(`/api/rooms/${encodeURIComponent(id)}/brief`, {
    method: 'POST',
    data: botId ? { botId } : {},
    parse: (value) => parseBrief(object(value).brief),
  })
}

export function editRoomBrief(id: string, brief: Pick<RoomBrief, 'decisions' | 'todos'>) {
  return requestJson(`/api/rooms/${encodeURIComponent(id)}/brief`, {
    method: 'PUT',
    data: brief,
    parse: (value) => parseBrief(object(value).brief),
  })
}
