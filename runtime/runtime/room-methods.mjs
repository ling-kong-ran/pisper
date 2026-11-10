// Bot 群聊的运行时编排（混入 AgentRuntimeService，同 agent-session-methods）：
// 你在群里说一句 → 算出叫醒谁 → 每个 Bot 在自己的席位会话里读没读过的那段、干活 → 这一轮的回复贴回群里
// （一轮一张工作卡：开始时贴出“在做”，结束时写上结果）→ 回复正文里 @ 了别的 Bot 就沿链叫醒它。
// 叫醒在席位会话忙时插进当前这一轮（v2.1 的不阻塞）；插不进去（正要开始或刚结束）就记下，这一轮结束后再叫。
// “读到第几条”送达后才前移：插队排进了，或新的一轮真正开始了。
import { randomUUID } from 'node:crypto'
import { isPisperError, pisperError } from '../../shared/errors.mjs'
import {
  roomAttachmentsNote,
  roomBriefInstruction,
  roomRouteInstruction,
  roomSeatPrompt,
  roomSkippedAttachmentsNote,
} from '../prompts/rooms.mjs'
import { MAX_BOT_LINK_HOPS, isCourtesyMessage, newBotChainId } from '../services/bots/bot-links.mjs'
import { botSourceTrust, stricterBotTrust } from '../services/bots/bot-memory-service.mjs'
import { toolArgsSummary } from '../services/session-card-summary.mjs'
import {
  archiveRoomAttachments,
  isRoomOnlyAsset,
  roomsStillUse,
  wakeAttachments,
} from '../services/rooms/room-attachments.mjs'
import {
  botWakeTargets,
  buildWake,
  routesToHost,
  userWakeTargets,
  wakeBudgetChars,
} from '../services/rooms/room-context.mjs'
import {
  ROOM_CHAIN_MAX_WAKES,
  ROOM_MESSAGE_ID_PATTERN,
  ROOM_ROUTE_MAX_WAKES,
  isPassReply,
  normalizeBrief,
  normalizeUserRoomMessage,
  parseBriefReply,
  parseMentions,
  roomAttachmentRefs,
  parseRouteReply,
} from '../services/rooms/room-model.mjs'
import { botTurnTrigger, requestBotText } from './bot-decisions.mjs'
import { localDayKey } from './conversation-memory-capture.mjs'
import { ATTACHMENT_MARKER, isInternalParentMessage } from './stream-projection.mjs'

// 插进正在跑的一轮时整句话（含上下文）不能超过 queueSessionMessage 的 12000 字。
const QUEUED_WAKE_MAX_CHARS = 10_000
const MAX_TURN_STEPS = 6
const MAX_CHAINS = 500
const BRIEF_SOURCE_CHARS = 60_000
const PAGE_LIMIT = 200
const MAX_WATCHED_CARDS = 50
// 主持人分派时带上最近几条（每条截到多长），以及这次请求最多等多久。
const ROUTE_TAIL_MESSAGES = 8
const ROUTE_TAIL_CHARS = 400
const ROUTE_TIMEOUT_MS = 60_000
// 同一条消息叫醒几个 Bot 时共用的附件内容留多久。
const ROOM_ATTACHMENT_SHARE_MS = 15_000

function unique(values) {
  return [...new Set(values)]
}

function oneLine(value, max) {
  const text = String(value ?? '')
    .replace(/\s+/g, ' ')
    .trim()
  return text.length > max ? `${text.slice(0, max)}…` : text
}

export const roomMethods = {
  // 群聊跟着 Bots：Runtime 能力与实验开关都开着才有。
  roomsEnabled() {
    return Boolean(
      this.rooms &&
      this.bots &&
      this.capabilities?.features?.bots &&
      this.isBetaFeatureEnabled?.('bots'),
    )
  },

  requireRoom(id) {
    if (!this.roomsEnabled())
      throw pisperError('unavailable', 'bot', 'turn on Bots in Settings › Labs', { status: 409 })
    return this.rooms.require(String(id || ''))
  },

  // 这个会话是不是某个 Bot 在某个群里的席位。
  roomSeatOf(sessionId) {
    if (!sessionId || !this.roomsEnabled()) return null
    return this.rooms.seatOfSession(sessionId)
  },

  // 席位会话的系统提示追加段（接在 Bot 对话提示后面）。
  roomSeatPromptFor(sessionId, bot) {
    const seat = this.roomSeatOf(sessionId)
    const room = seat ? this.rooms.get(seat.roomId) : null
    return room ? roomSeatPrompt(room, bot) : ''
  },

  roomMembers(room) {
    return room.members.map((id) => {
      const bot = this.bots.get(id)
      return bot
        ? {
            id,
            name: bot.name,
            instructions: bot.instructions,
            enabled: bot.enabled,
            paused: !bot.enabled,
            avatar: bot.avatar,
          }
        : { id, name: '', instructions: '', enabled: false, paused: false, deleted: true }
    })
  },

  // 一个席位此刻在干嘛：空闲 / 在做 / 等你批准或回答。current 是正在跑的那一步（工具名 + 参数摘要）。
  seatStatus(sessionId) {
    if (!sessionId) return { state: 'idle', waiting: null, current: '' }
    const running = this.sessionRunIsActive?.(sessionId) || this.startingRuns?.has(sessionId)
    const { approval, question } = this.botWaiting(sessionId)
    const waiting = approval ? 'approval' : question ? 'question' : null
    const activity = this.sessionCards?.entry(sessionId)?.activity
    const current =
      activity?.type === 'tool'
        ? oneLine(`${activity.name}${activity.argsSummary ? ` ${activity.argsSummary}` : ''}`, 120)
        : ''
    return {
      state: waiting ? 'waiting' : running ? 'running' : 'idle',
      waiting,
      current,
      ...(approval ? { approval } : {}),
      ...(question ? { question } : {}),
    }
  },

  roomActivity(roomId) {
    let working = 0
    let needsYou = false
    for (const { sessionId } of this.rooms.sessionsOfRoom(roomId)) {
      const status = this.seatStatus(sessionId)
      if (status.state !== 'idle') working += 1
      if (status.waiting) needsYou = true
    }
    return { working, needsYou }
  },

  // —— 推送：看板事件流的 room 帧只带群 id、最后一条的序号和变化范围，页面再去取增量 ——

  // RoomService 写了东西（消息、补丁、撤回、席位、简报、群设定）。
  roomStoreChanged(roomId, scope) {
    if (this.roomsEnabled()) this.boardEvents?.roomChanged(roomId, { scope })
  },

  // 看板集线器的旁听：每个会话的每次变化都会来（流事件、审批、提问），先用一次 Map 查找筛出席位会话。
  roomSeatTouched(sessionId) {
    const seat = this.rooms?.seatOfSession(sessionId)
    if (!seat || !this.roomsEnabled()) return
    this.boardEvents?.roomChanged(seat.roomId, {
      scope: ['messages', 'detail', 'list'],
      probe: true,
    })
  },

  // 群页面上会跟着席位变的那部分（状态、当前这一步、等你批准或回答什么、走到第几步）的签名：
  // 集线器拿它判断一次探测是不是真有变化，Bot 一边说话一边吐字时不至于每秒推四帧。
  roomLiveState(roomId) {
    if (!this.roomsEnabled() || !this.rooms.get(roomId)) return null
    const seats = this.rooms
      .sessionsOfRoom(roomId)
      .map(({ sessionId, botId }) => [
        botId,
        this.seatStatus(sessionId),
        this.roomSeatTurns?.get(sessionId)?.stepCount || 0,
      ])
    return {
      lastSeq: this.rooms.peekLastSeq(roomId),
      signature: JSON.stringify([seats, Boolean(this.roomBriefRuns?.has(roomId))]),
    }
  },

  listRooms() {
    if (!this.roomsEnabled()) return []
    return this.rooms.list().map((room) => ({ ...room, activity: this.roomActivity(room.id) }))
  },

  // 群详情：成员（各自的状态、读到哪、还有几条没读）、简报、用量。
  async getRoom(id) {
    const room = this.requireRoom(id)
    const [state, messages] = await Promise.all([this.rooms.state(id), this.rooms.peekMessages(id)])
    const members = this.roomMembers(room).map((member) => {
      const seat = state.seats[member.id] || null
      const unread = messages.filter(
        (message) =>
          message.seq > (seat?.readSeq || 0) &&
          !(message.author?.kind === 'bot' && message.author.botId === member.id) &&
          message.author?.kind !== 'system' &&
          // 跳过的那张卡没有内容可读。
          message.turn?.state !== 'skipped',
      ).length
      return {
        id: member.id,
        name: member.name,
        avatar: member.avatar || null,
        enabled: member.enabled,
        deleted: Boolean(member.deleted),
        sessionId: seat?.sessionId || '',
        readSeq: seat?.readSeq || 0,
        unread,
        status: this.seatStatus(seat?.sessionId || ''),
      }
    })
    let totalTokens = 0
    for (const member of members) {
      const usage =
        this.sessionCards?.entry(member.sessionId)?.totalTokens ??
        this.sessionMeta?.[member.sessionId]?.boardCard?.usage?.totalTokens
      if (Number.isFinite(usage)) totalTokens += usage
    }
    return {
      room,
      members,
      brief: state.brief,
      lastSeq: messages.at(-1)?.seq || 0,
      activity: this.roomActivity(id),
      usage: { totalTokens },
      briefing: Boolean(this.roomBriefRuns?.has(id)),
    }
  },

  async validRoomInput(input) {
    const value = input && typeof input === 'object' ? { ...input } : {}
    if (Array.isArray(value.members))
      for (const botId of value.members)
        if (!this.bots.get(String(botId))) throw pisperError('not_found', 'bot')
    if (typeof value.cwd === 'string' && value.cwd.trim()) {
      try {
        // 和 Bot 的目录同一套校验：存在、可访问，解析成绝对路径。
        value.cwd = await this.bots.validateDirectory(value.cwd.trim())
      } catch {
        throw pisperError('not_found', 'folder', 'missing or not accessible', { status: 400 })
      }
    }
    return value
  },

  async createRoom(input) {
    this.requireRoomsOn()
    return this.rooms.create(await this.validRoomInput(input))
  },

  requireRoomsOn() {
    if (!this.roomsEnabled())
      throw pisperError('unavailable', 'bot', 'turn on Bots in Settings › Labs', { status: 409 })
  },

  async updateRoom(id, input) {
    const before = this.requireRoom(id)
    const room = await this.rooms.update(id, await this.validRoomInput(input))
    // 群改名：席位会话的标题（“小克 · 群名”）跟着改。
    if (room && room.name !== before.name)
      for (const { sessionId, botId } of this.rooms.sessionsOfRoom(id)) {
        const bot = this.bots.get(botId)
        if (bot) await this.refreshSeat(room, bot, sessionId, { settings: false }).catch(() => {})
      }
    // 群目录变了：已有的席位会话跟着换（运行中的换不了，下次空闲时 updateBot 或再次改群设置时补上）。
    if (room && room.cwd !== before.cwd)
      for (const { sessionId, botId } of this.rooms.sessionsOfRoom(id)) {
        const bot = this.bots.get(botId)
        if (bot)
          await this.setSessionCwd(sessionId, await this.seatSessionCwd(room, bot)).catch(() => {})
      }
    return room
  },

  async deleteRoom(id) {
    this.requireRoom(id)
    for (const { sessionId } of this.rooms.sessionsOfRoom(id))
      if (this.sessionRunIsActive?.(sessionId) || this.startingRuns?.has(sessionId))
        throw pisperError('busy', 'room', 'a bot is still working in it; stop it first')
    // 群里上传、别处没用到的附件跟着群一起删掉。
    const refs = (await this.rooms.peekMessages(id)).flatMap((message) => message.attachments || [])
    const removed = await this.rooms.remove(id)
    await this.releaseRoomAttachments(refs).catch(() => {})
    return removed
  },

  // 消息投影：进行中的工作卡带上席位此刻的状态（在做哪一步、在等你批准什么）。这一轮已经结束、但它派出去的活
  // 还在等你批准或回答时，挂在这个 Bot 最近的那张卡上（pending），批准不用跑去别处找。
  projectRoomMessage(message, latest = null) {
    const sessionId = message.turn?.sessionId
    if (!sessionId) return message
    const running = message.turn.state === 'running'
    if (!running && latest?.get(sessionId) !== message.id) return message
    const status = this.seatStatus(sessionId)
    if (!running) {
      if (!status.waiting) return message
      return {
        ...message,
        turn: {
          ...message.turn,
          pending: {
            waiting: status.waiting,
            ...(status.approval ? { approval: status.approval } : {}),
            ...(status.question ? { question: status.question } : {}),
          },
        },
      }
    }
    const live = this.roomSeatTurns?.get(sessionId)
    return {
      ...message,
      turn: {
        ...message.turn,
        ...(status.waiting ? { state: 'waiting', waiting: status.waiting } : {}),
        current: status.current,
        steps: live?.steps || message.turn.steps || [],
        stepCount: live?.stepCount || 0,
        ...(status.approval ? { approval: status.approval } : {}),
        ...(status.question ? { question: status.question } : {}),
      },
    }
  },

  // 每个席位最近的那张工作卡：sessionId → messageId。
  latestRoomCards(messages) {
    const latest = new Map()
    for (const message of messages)
      if (message.turn?.sessionId) latest.set(message.turn.sessionId, message.id)
    return latest
  },

  // 分页读消息：默认最后一页；before 往前翻；after 取某条之后的（轮询增量，同时带回还在进行中的工作卡
  // 与每个 Bot 最近的那张卡：它们的状态会变）。watch 是页面上还显示“在做”的卡：它们在页面没刷的时候
  // 做完了、又不是最近的那张时，靠这个带回最后的结果，不然页面上永远转圈。
  async listRoomMessages(id, { before, after, limit, watch } = {}) {
    this.requireRoom(id)
    // 只读：投影出来的都是新对象，不改记录本身。
    const messages = await this.rooms.peekMessages(id)
    const latest = this.latestRoomCards(messages)
    const latestIds = new Set(latest.values())
    const size = Math.min(Math.max(Number(limit) || 50, 1), PAGE_LIMIT)
    let page
    let hasMore = false
    if (Number.isSafeInteger(Number(after)) && after !== undefined && after !== '') {
      const from = Number(after)
      const newer = messages.filter((message) => message.seq > from)
      const watched = new Set(
        String(watch || '')
          .split(',')
          .filter((item) => ROOM_MESSAGE_ID_PATTERN.test(item))
          .slice(0, MAX_WATCHED_CARDS),
      )
      const live = messages.filter(
        (message) =>
          message.seq <= from &&
          (message.turn?.state === 'running' ||
            latestIds.has(message.id) ||
            watched.has(message.id)),
      )
      page = [...live, ...newer.slice(0, PAGE_LIMIT)]
      hasMore = newer.length > PAGE_LIMIT
    } else {
      const upper = Number(before) > 0 ? Number(before) : Infinity
      const older = messages.filter((message) => message.seq < upper)
      page = older.slice(-size)
      hasMore = older.length > size
    }
    return {
      messages: page.map((message) => this.projectRoomMessage(message, latest)),
      lastSeq: messages.at(-1)?.seq || 0,
      hasMore,
    }
  },

  // 你在群里发一句话。
  async postRoomMessage(id, input) {
    const room = this.requireRoom(id)
    const messages = await this.rooms.messages(id)
    const byId = new Map(messages.map((message) => [message.id, message]))
    const normalized = normalizeUserRoomMessage(input, room.members, (messageId) =>
      byId.has(messageId),
    )
    const members = this.roomMembers(room)
    const mentions = unique([
      ...normalized.mentions,
      ...parseMentions(
        normalized.text,
        members.filter((member) => member.name),
      ),
    ])
    const replied = normalized.replyTo ? byId.get(normalized.replyTo) : null
    const chain = { id: newBotChainId(), hop: 1 }
    const attachments = await this.storeRoomAttachments(room, normalized.attachments)
    // 写进记录（或没写成）之后，这些资产不再算“正在用”：之后由记录里的引用说了算。
    const unhold = () => {
      for (const ref of attachments) if (ref.id) this.holdRoomAsset(ref.id, -1)
    }
    const message = await this.rooms
      .append(id, {
        author: { kind: 'user' },
        text: normalized.text,
        mentions,
        ...(attachments.length ? { attachments } : {}),
        ...(normalized.replyTo ? { replyTo: normalized.replyTo } : {}),
        ...(normalized.recipients === 'all' || normalized.recipients.length
          ? { recipients: normalized.recipients }
          : {}),
        chain,
      })
      .catch(async (error) => {
        unhold()
        await this.releaseRoomAttachments(attachments).catch(() => {})
        throw error
      })
    unhold()
    // “发给”的选择留在群里（像收件人），下次打开还在。
    if (
      input &&
      Object.hasOwn(input, 'recipients') &&
      JSON.stringify(normalized.recipients) !== JSON.stringify(room.recipients)
    )
      await this.rooms.update(id, { recipients: normalized.recipients })
    const { wake, paused } = userWakeTargets(
      {
        ...normalized,
        mentions,
        replyToAuthor: replied?.author?.kind === 'bot' ? replied.author.botId : '',
      },
      members,
    )
    for (const botId of paused)
      await this.rooms.append(id, {
        author: { kind: 'system' },
        event: { type: 'paused', botId },
      })
    for (const target of wake)
      void this.wakeSeat(id, target.botId, {
        trigger: message,
        reason: { kind: target.kind, seq: message.seq },
        chain,
        trust: 'user',
      }).catch(() => console.warn('[rooms] Could not wake the bot.'))
    // 主持人分派：没点名的话交给主持人判断该谁答。在后台做，不拖住这次发送的响应。
    const routing = routesToHost(
      {
        ...normalized,
        mentions,
        replyToAuthor: replied?.author?.kind === 'bot' ? replied.author.botId : '',
      },
      room,
    )
    if (routing) void this.routeRoomMessage(id, message, chain)
    return { message, woke: wake.map((target) => target.botId), ...(routing ? { routing } : {}) }
  },

  // 同一个群的分派按顺序来：连着发几句没点名的话时，主持人的判断按发出的顺序贴出来。永不抛错。
  routeRoomMessage(roomId, message, chain) {
    this.roomRouteQueues ||= new Map()
    const previous = this.roomRouteQueues.get(roomId) || Promise.resolve()
    const next = previous
      .then(() => this.routeRoomMessageNow(roomId, message, chain))
      .catch(() => console.warn('[rooms] Could not route the message to the host.'))
      .finally(() => {
        if (this.roomRouteQueues.get(roomId) === next) this.roomRouteQueues.delete(roomId)
      })
    this.roomRouteQueues.set(roomId, next)
    return next
  },

  // 主持人用自己的模型做一次不带工具的独立请求（同简报），只回 JSON：该谁答、一句理由。
  // 选中的照常经 wakeSeat 叫醒（链沿用你这句话的）；谁也没选、请求失败或回复不对，都只在群里写一行，不叫人。
  async routeRoomMessageNow(roomId, message, chain) {
    const room = this.rooms.get(roomId)
    if (!room || room.answerMode !== 'host' || !this.roomsEnabled()) return
    const hostId = room.hostBotId
    const note = (event) =>
      this.rooms.append(roomId, {
        author: { kind: 'system' },
        event: { botId: hostId, seq: message.seq, ...event },
      })
    const host = this.bots.get(hostId)
    if (!host || !room.members.includes(hostId))
      return note({ type: 'route_failed', reason: 'no_host' })
    if (!host.enabled) return note({ type: 'route_failed', reason: 'host_paused' })
    // 能被请来的：还在群里、没暂停的成员（主持人自己也可以）。
    const eligible = this.roomMembers(room).filter(
      (member) => member.name && !member.deleted && member.enabled,
    )
    // 只读：不改记录本身。
    const messages = await this.rooms.peekMessages(roomId)
    const names = (author) =>
      author?.kind === 'bot'
        ? eligible.find((member) => member.id === author.botId)?.name || 'a removed bot'
        : 'user'
    const lines = messages
      .filter(
        (item) =>
          item.seq < message.seq && !item.recalled && item.text && item.author?.kind !== 'system',
      )
      .slice(-ROUTE_TAIL_MESSAGES)
      .map((item) => `#${item.seq} ${names(item.author)}: ${oneLine(item.text, ROUTE_TAIL_CHARS)}`)
    let result
    const timeout = AbortSignal.timeout(ROUTE_TIMEOUT_MS)
    try {
      const model = await this.roomBriefModel(roomId, host)
      if (!model) throw pisperError('unavailable', 'model')
      result = await requestBotText({
        modelRuntime: this.modelRuntime,
        model,
        text: roomRouteInstruction({
          room,
          host,
          members: eligible,
          lines,
          message,
          max: ROOM_ROUTE_MAX_WAKES,
        }),
        signal: timeout,
      })
    } catch {
      return note({ type: 'route_failed', reason: 'failed' })
    }
    if (result.usage)
      await this.recordUsage?.(
        localDayKey(),
        `room-route:${roomId}:${randomUUID()}`,
        result.usage,
      ).catch(() => {})
    // 请求出错或超时（没有回复）是“没问成”，不是“回复不对”。
    if (result.text === null || timeout.aborted)
      return note({ type: 'route_failed', reason: 'failed' })
    const parsed = parseRouteReply(result.text, eligible)
    if (!parsed) return note({ type: 'route_failed', reason: 'invalid' })
    // 等主持人的这一会儿，这句话被撤回了、群被删了或改回“@ 谁谁答”：不再叫人。
    const current = this.rooms.get(roomId)
    const still = await this.rooms.message(roomId, message.id)
    if (!current || current.answerMode !== 'host' || !still || still.recalled) return
    await note({ type: 'routed', bots: parsed.bots, note: parsed.reason })
    for (const botId of parsed.bots)
      void this.wakeSeat(roomId, botId, {
        trigger: message,
        reason: { kind: 'host', seq: message.seq, host: host.name, why: parsed.reason },
        chain,
        trust: 'user',
      }).catch(() => console.warn('[rooms] Could not wake the bot.'))
  },

  // 记下要叫醒某个席位，然后尽快送进去。同一个席位的送达按顺序来（不会建出两条席位会话、不会并发开两轮）。
  wakeSeat(roomId, botId, wake) {
    this.roomWakes ||= new Map()
    this.roomChainWakes ||= new Map()
    const key = `${roomId}:${botId}`
    const pending = this.roomWakes.get(key) || {
      triggers: [],
      reasons: [],
      notes: [],
      chain: null,
      trust: 'user',
    }
    if (wake.trigger) pending.triggers.push(wake.trigger.id)
    if (wake.note) pending.notes.push(wake.note)
    pending.reasons.push(wake.reason)
    pending.chain ||= wake.chain || null
    // 一次送进去的几句话里按最不可信的那句算（别的 Bot 说的话不能冒充你说的）。
    pending.trust = stricterBotTrust(pending.trust, wake.trust || 'user')
    this.roomWakes.set(key, pending)
    if (wake.chain?.id) {
      this.roomChainWakes.set(wake.chain.id, (this.roomChainWakes.get(wake.chain.id) || 0) + 1)
      while (this.roomChainWakes.size > MAX_CHAINS)
        this.roomChainWakes.delete(this.roomChainWakes.keys().next().value)
    }
    return this.flushSeatWake(roomId, botId)
  },

  flushSeatWake(roomId, botId) {
    this.roomSeatQueues ||= new Map()
    const key = `${roomId}:${botId}`
    const previous = this.roomSeatQueues.get(key) || Promise.resolve()
    const next = previous
      .catch(() => {})
      .then(() => this.deliverSeatWake(roomId, botId))
      .finally(() => {
        if (this.roomSeatQueues.get(key) === next) this.roomSeatQueues.delete(key)
      })
    this.roomSeatQueues.set(key, next)
    return next
  },

  async seatSessionCwd(room, bot) {
    // 群目录只对本机的 Bot 生效，只是起始目录；执行节点上的 Bot 仍在它自己的电脑和目录里。
    if ((bot.workspace.environmentId || 'local') === 'local' && room.cwd) return room.cwd
    return this.botSessionCwd(bot)
  },

  // 这个 Bot 在这个群里的席位会话：没有（或会话文件已经不在）就新建，设定都来自这个 Bot。
  // 同一个席位同时只建一次：定时到点和你 @ 它撞在一起时，不会各建一条、后写的把先建的顶成孤儿。
  ensureSeatSession(room, bot) {
    this.roomSeatCreates ||= new Map()
    const key = `${room.id}:${bot.id}`
    const running = this.roomSeatCreates.get(key)
    if (running) return running
    const task = this.createSeatSession(room, bot).finally(() => {
      if (this.roomSeatCreates.get(key) === task) this.roomSeatCreates.delete(key)
    })
    this.roomSeatCreates.set(key, task)
    return task
  },

  async createSeatSession(room, bot) {
    const seat = await this.rooms.seat(room.id, bot.id)
    const existing = seat?.sessionId
    if (
      existing &&
      (this.sessions.has(existing) ||
        this.pendingSessions.has(existing) ||
        (await this.findSessionInfo(existing)))
    )
      return existing
    const cwd = await this.seatSessionCwd(room, bot)
    const created = await this.createSession(`${bot.name} · ${room.name}`, cwd)
    // 先登记席位再套设定：第一轮装配运行时（botForSession）时就得认出它是这个 Bot 的会话。
    await this.rooms.setSeat(room.id, bot.id, { sessionId: created.id })
    await this.applyBotSettings(bot, created.id, { cwd })
    await this.rooms.setSeat(room.id, bot.id, {
      applied: String(bot.updatedAt || ''),
      title: `${bot.name} · ${room.name}`,
    })
    return created.id
  },

  // 席位会话跟上 Bot 的最新设定与名字。套设定要装配这条会话的运行时，所以只在用得上的时候做：
  // 会话已经在内存里（改完立即生效），或它正要被叫醒；别的席位等下次叫醒。改名不用装配，随时做。
  async refreshSeat(room, bot, sessionId, { settings = true } = {}) {
    const seat = await this.rooms.seat(room.id, bot.id)
    const stamp = String(bot.updatedAt || '')
    const title = `${bot.name} · ${room.name}`
    const patch = {}
    if (settings && seat?.applied !== stamp) {
      await this.applyBotSettings(bot, sessionId, { cwd: await this.seatSessionCwd(room, bot) })
      patch.applied = stamp
    }
    if (seat?.title !== title && this.renameSession) {
      // 正在跑时改不了标题：留着，下次再改。
      const renamed = await this.renameSession(sessionId, title).then(
        () => true,
        () => false,
      )
      if (renamed) patch.title = title
    }
    if (Object.keys(patch).length) await this.rooms.setSeat(room.id, bot.id, patch)
  },

  // Bot 的设定或名字变了：同步到它在各个群里的席位。
  async syncBotSeats(bot) {
    for (const seat of this.rooms?.sessionsOfBot(bot.id) || []) {
      const room = this.rooms.get(seat.roomId)
      if (!room) continue
      await this.refreshSeat(room, bot, seat.sessionId, {
        settings: this.sessions.has(seat.sessionId),
      }).catch(() => console.warn('[rooms] Could not apply the bot settings to its seat.'))
    }
  },

  async deliverSeatWake(roomId, botId) {
    const key = `${roomId}:${botId}`
    const pending = this.roomWakes?.get(key)
    if (!pending) return
    const room = this.rooms.get(roomId)
    const bot = this.bots.get(botId)
    if (!room || !bot || !room.members.includes(botId) || !bot.enabled || !this.roomsEnabled()) {
      this.roomWakes.delete(key)
      return
    }
    const sessionId = await this.ensureSeatSession(room, bot)
    const isBusy = () => this.sessionRunIsActive(sessionId) || this.startingRuns.has(sessionId)
    const value = this.sessions.get(sessionId)
    const busy = isBusy()
    // 正在开始（还没进入流式）：等这一轮结束后的 flush。
    if (busy && !value?.session?.isStreaming) return
    // 要新开一轮：先让这条席位跟上 Bot 的最新设定（改设定时没加载的席位是留到这里才套的）。
    if (!busy) await this.refreshSeat(room, bot, sessionId).catch(() => {})
    const window = value?.session?.model?.contextWindow
    const budget = busy ? QUEUED_WAKE_MAX_CHARS : wakeBudgetChars(window)
    const wake = await this.composeSeatWake(room, bot, pending, budget)
    // 拼上下文的这一会儿，席位可能被别的来源开了一轮（子任务做完的通知、你直接在席位会话里说话）
    // 或刚好说完：按新的状态重来，不拿过期的判断去送。
    if (isBusy() !== busy) return this.deliverSeatWake(roomId, botId)
    if (busy) {
      try {
        await this.queueSessionMessage(sessionId, {
          message: wake.message,
          attachments: wake.attachments,
          behavior: 'interrupt',
        })
      } catch {
        // 恰好在这一刻说完了：留着，这一轮的收尾会再 flush。
        return
      }
      this.roomWakes.delete(key)
      await this.commitSeatRead(roomId, botId, wake)
      const turn = this.roomSeatTurns?.get(sessionId)
      if (turn) {
        turn.alsoAnswered.push(...pending.triggers)
        // 插进来的是 @所有人：这一轮也可以用 [pass] 表示没什么要说的。
        if (pending.reasons.some((reason) => reason.kind === 'everyone')) turn.everyone = true
        turn.chain ||= pending.chain || null
      }
      if (value) value.botTurnTrust = stricterBotTrust(value.botTurnTrust, pending.trust)
      return
    }
    this.roomWakes.delete(key)
    this.roomSeatStarts ||= new Map()
    const start = { roomId, botId, pending, wake, message: wake.message }
    this.roomSeatStarts.set(sessionId, start)
    this.botPendingTrust?.set(sessionId, pending.trust)
    if (wake.needsBrief && room.briefBotId)
      void this.refreshRoomBrief(roomId, { auto: true }).catch(() => {})
    void this.promptFromChannel({
      sessionId,
      message: wake.message,
      attachments: wake.attachments,
      unattended: pending.trust !== 'user',
    }).catch(async (error) => {
      // 这一轮已经开始了（工作卡贴出来了）：失败写在卡上，这里不用管。
      if (this.roomSeatStarts.get(sessionId) !== start) return
      this.roomSeatStarts.delete(sessionId)
      this.botPendingTrust?.delete(sessionId)
      if (error?.code === 'busy') {
        // 被别的来源抢先开了一轮：这次叫醒没送进去，放回去；那一轮结束时会再送，已经结束了就现在送。
        this.restoreSeatWake(key, pending)
        if (!isBusy()) void this.flushSeatWake(roomId, botId)
        return
      }
      // 写明是哪一类原因（附件读不了、模型用不了），只给类别，不带出错信息里的路径。
      const subject = isPisperError(error) ? String(error.params?.subject || '') : ''
      const reason = ['attachment', 'image', 'file'].includes(subject)
        ? 'attachment'
        : subject === 'model' || subject === 'provider'
          ? 'model'
          : ''
      await this.rooms
        .append(roomId, {
          author: { kind: 'system' },
          event: { type: 'wake_failed', botId, ...(reason ? { reason } : {}) },
        })
        .catch(() => {})
      // 开不起来的这会儿又来了新的叫醒（它在等“这一轮结束”）：没有哪一轮会结束了，现在就送。
      if (this.roomWakes.has(key) && !isBusy()) void this.flushSeatWake(roomId, botId)
    })
  },

  // 没送进去的叫醒放回待送（排在这期间新来的前面）。
  restoreSeatWake(key, pending) {
    const later = this.roomWakes.get(key)
    this.roomWakes.set(
      key,
      later
        ? {
            triggers: [...pending.triggers, ...later.triggers],
            reasons: [...pending.reasons, ...later.reasons],
            notes: [...pending.notes, ...later.notes],
            chain: pending.chain || later.chain,
            trust: stricterBotTrust(pending.trust, later.trust),
          }
        : pending,
    )
  },

  // 送达后记下读到哪：越过去的进行中的卡记进 late（写完后补送），这次告知过的撤回划掉。
  // 更早算出的那次晚到时不覆盖已经记下的进度。
  async commitSeatRead(roomId, botId, wake) {
    const seat = await this.rooms.seat(roomId, botId)
    if (seat && seat.readSeq > wake.upToSeq) return
    await this.rooms.setSeat(roomId, botId, {
      readSeq: wake.upToSeq,
      late: wake.late,
      recalls: (seat?.recalls || []).filter((seq) => !wake.recalled.includes(seq)),
      lastWakeAt: new Date().toISOString(),
    })
  },

  // 叫醒时送进席位会话的那句话：看得见的是触发它的消息原文（或定时、回答的原话），群上下文藏在附件上下文里。
  async composeSeatWake(room, bot, pending, budget) {
    const [messages, seat, state] = await Promise.all([
      this.rooms.messages(room.id),
      this.rooms.seat(room.id, bot.id),
      this.rooms.state(room.id),
    ])
    const members = this.roomMembers(room).filter((member) => member.name)
    const names = (author) =>
      author?.kind === 'bot'
        ? members.find((member) => member.id === author.botId)?.name || 'a removed bot'
        : author?.kind === 'system'
          ? 'system'
          : 'user'
    // 排队期间被撤回的触发消息不再原样送进去。
    const triggers = pending.triggers
      .map((id) => messages.find((message) => message.id === id))
      .filter((message) => message && !message.recalled)
    // 叫醒它的话带着附件：内容交给席位会话（和会话里发附件一样），只带附件没写字时看得见的是文件名。
    const files = await wakeAttachments(
      triggers.filter((message) => message.author?.kind === 'user'),
      (assetId) => this.getAssetDownload(assetId),
      this.sharedRoomAttachmentReads(),
    )
    const visible =
      [
        ...triggers.map((message) =>
          message.author?.kind === 'bot'
            ? `${names(message.author)}: ${message.text}`
            : String(message.text || '') ||
              roomAttachmentsNote(roomAttachmentRefs(message.attachments).map((ref) => ref.name)),
        ),
        ...pending.notes,
      ]
        .join('\n\n')
        .slice(0, 4_000) || '…'
    const result = buildWake({
      room,
      bot,
      members,
      messages,
      seat,
      brief: state.brief,
      reasons: pending.reasons,
      budget: Math.max(2_000, budget - visible.length - 200),
      names,
    })
    const skipped = roomSkippedAttachmentsNote(files.skipped)
    return {
      message: `${visible}${ATTACHMENT_MARKER}${result.context}${skipped ? `\n\n${skipped}` : ''}`,
      attachments: files.attachments,
      upToSeq: result.upToSeq,
      late: result.late,
      recalled: result.recalled,
      needsBrief: result.needsBrief,
    }
  },

  // 席位会话的一轮开始（streamPrompt 进入运行前调用）：贴出“在做”的工作卡，送达的那段记为已读。
  // message 是这一轮送进会话的那句话：只有它正是我们送的那次叫醒，才算“送达”。别的来源抢先开的一轮
  // （子任务做完的通知、你直接在席位会话里说话）不能把还没送进去的叫醒记成已读。
  onSeatRunStart(sessionId, message) {
    const seat = this.roomSeatOf(sessionId)
    if (!seat) return
    this.roomSeatStarts ||= new Map()
    this.roomSeatTurns ||= new Map()
    this.roomSeatChains ||= new Map()
    const waiting = this.roomSeatStarts.get(sessionId)
    const start = waiting && (message == null || waiting.message === message) ? waiting : null
    if (start) this.roomSeatStarts.delete(sessionId)
    const value = this.sessions.get(sessionId)
    const model = value?.session?.model
      ? `${value.session.model.provider}/${value.session.model.id}`
      : ''
    const pending = start?.pending
    const turn = {
      roomId: seat.roomId,
      botId: seat.botId,
      messageId: '',
      // 子任务做完后的汇报接着派它出去的那一轮的链算：不然“汇报里 @ 对方 → 对方再派活 → 它的汇报又 @ 回来”
      // 每次都是一条新链，总共 12 次的上限就挡不住了。你直接在席位会话里说的话仍是新的开始。
      chain:
        pending?.chain ||
        (!pending && isInternalParentMessage(String(message || ''))
          ? this.roomSeatChains.get(sessionId)
          : null) ||
        null,
      everyone: Boolean(pending?.reasons.some((reason) => reason.kind === 'everyone')),
      alsoAnswered: [],
      steps: [],
      stepCount: 0,
      ready: null,
    }
    this.roomSeatTurns.set(sessionId, turn)
    if (turn.chain) {
      this.roomSeatChains.delete(sessionId)
      this.roomSeatChains.set(sessionId, turn.chain)
      while (this.roomSeatChains.size > MAX_CHAINS)
        this.roomSeatChains.delete(this.roomSeatChains.keys().next().value)
    }
    turn.ready = (async () => {
      const message = await this.rooms.append(seat.roomId, {
        author: { kind: 'bot', botId: seat.botId },
        text: '',
        mentions: [],
        ...(pending?.triggers.length ? { replyTo: pending.triggers.at(-1) } : {}),
        turn: {
          id: randomUUID(),
          sessionId,
          state: 'running',
          // 没有群里的触发（子 agent 做完、定时、你在席位会话里直接说的）：汇报。
          reason: pending ? (pending.triggers.length ? 'wake' : 'note') : 'report',
          triggers: pending?.triggers || [],
          model,
          startedAt: new Date().toISOString(),
        },
        ...(turn.chain ? { chain: turn.chain } : {}),
      })
      turn.messageId = message.id
      if (start) await this.commitSeatRead(seat.roomId, seat.botId, start.wake)
    })().catch(() => console.warn('[rooms] Could not post the work card.'))
  },

  // 席位会话里的运行事件：工作卡上列最近几步（工具名 + 参数摘要）。
  observeSeatEvent(sessionId, event, data) {
    if (event !== 'tool_start') return
    const turn = this.roomSeatTurns?.get(sessionId)
    if (!turn) return
    const name = oneLine(data?.name, 60) || 'tool'
    const summary = toolArgsSummary(name, data?.args)
    turn.stepCount += 1
    turn.steps = [...turn.steps, { name, summary: oneLine(summary, 120) }].slice(-MAX_TURN_STEPS)
  },

  // 这一轮结束（完成、失败、停止都算）：写上结果；正文里 @ 了别的 Bot 就沿链叫醒；没送进去的叫醒补上。
  async onSeatRunEnd(sessionId, { error = '', aborted = false } = {}) {
    const turn = this.roomSeatTurns?.get(sessionId)
    if (!turn) {
      // 这一轮没开起来（比如模型不可用）：没有卡要收，但等着“这一轮结束后再送”的叫醒不能一直挂着。
      const seat = this.roomSeatOf(sessionId)
      if (seat && this.roomWakes?.has(`${seat.roomId}:${seat.botId}`))
        void this.flushSeatWake(seat.roomId, seat.botId)
      return
    }
    this.roomSeatTurns.delete(sessionId)
    await turn.ready
    const room = this.rooms.get(turn.roomId)
    if (!turn.messageId || !room) {
      if (room) void this.flushSeatWake(turn.roomId, turn.botId)
      return
    }
    const value = this.sessions.get(sessionId)
    const reply = botTurnTrigger(value?.session?.messages || []).reply
    const state = aborted
      ? 'stopped'
      : error
        ? 'error'
        : turn.everyone && isPassReply(reply)
          ? 'skipped'
          : 'done'
    const members = this.roomMembers(room).filter((member) => member.name)
    const mentions =
      state === 'done' ? parseMentions(reply, members).filter((id) => id !== turn.botId) : []
    const card = this.sessionCards?.entry(sessionId)
    const changes =
      card?.wroteFiles && card.changeStats?.files
        ? {
            files: card.changeStats.files,
            added: card.changeStats.added ?? null,
            removed: card.changeStats.removed ?? null,
          }
        : null
    const current = await this.rooms.message(turn.roomId, turn.messageId)
    const message = await this.rooms.patch(turn.roomId, turn.messageId, {
      text: state === 'done' ? reply : '',
      mentions,
      turn: {
        ...(current?.turn || {}),
        state,
        endedAt: new Date().toISOString(),
        steps: turn.steps,
        stepCount: turn.stepCount,
        ...(turn.alsoAnswered.length ? { alsoAnswered: unique(turn.alsoAnswered) } : {}),
        ...(changes ? { changes } : {}),
        ...(state === 'error' ? { error: oneLine(error, 300) } : {}),
      },
    })
    if (message && state === 'done') {
      const bot = this.bots.get(turn.botId)
      for (const target of botWakeTargets({ authorId: turn.botId, mentions }, members))
        await this.wakeFromBot(room, bot, target, message, turn.chain)
    }
    void this.flushSeatWake(turn.roomId, turn.botId)
  },

  // Bot 在群里 @ 了另一个 Bot：走 Bot 之间的限制（6 跳、每对每分钟、客气话不叫），外加一条链总共 12 次。
  async wakeFromBot(room, from, toId, message, chain) {
    if (!from) return { woke: false, reason: 'no sender' }
    // 去掉 @名字 再看是不是只有一句客气话（“@小克 谢谢”不值得叫醒一轮）。
    let said = String(message.text || '')
    const names = this.roomMembers(room)
      .map((member) => member.name)
      .filter(Boolean)
      .sort((left, right) => right.length - left.length)
    for (const name of names) said = said.replaceAll(`@${name}`, ' ')
    if (isCourtesyMessage(said)) return { woke: false, reason: 'courtesy only' }
    const chainId = chain?.id || newBotChainId()
    const hop = (chain?.hop || 0) + 1
    const total = this.roomChainWakes?.get(chainId) || 0
    const stop = async (reason) => {
      await this.rooms.append(room.id, {
        author: { kind: 'system' },
        event: { type: 'chain_stopped', reason, from: from.id, to: toId, wakes: total },
      })
      return { woke: false, reason: `chain limit: ${reason}` }
    }
    if (total >= ROOM_CHAIN_MAX_WAKES) return stop('total')
    const admitted = this.botLinks.admit({ from: from.id, to: toId, chain: chainId, hop })
    if (!admitted.ok) return stop(admitted.reason)
    await this.wakeSeat(room.id, toId, {
      trigger: message,
      reason: { kind: 'bot', seq: message.seq, from: from.name, hop, maxHops: MAX_BOT_LINK_HOPS },
      chain: { id: chainId, hop },
      trust: botSourceTrust({ kind: 'bot' }),
    })
    return { woke: true, reason: '' }
  },

  // 席位会话里的 bot_message：话留在群里（作者是这个 Bot，正文 @ 对方），再按链叫醒对方。
  async postBotRoomMessage(sessionId, from, target, text) {
    const seat = this.roomSeatOf(sessionId)
    const room = seat ? this.rooms.get(seat.roomId) : null
    if (!room) throw new Error('This seat no longer belongs to a room.')
    if (!room.members.includes(target.id))
      throw new Error(`${target.name} is not in this room; ask the user to add it.`)
    const turn = this.roomSeatTurns?.get(sessionId)
    const body = String(text || '').includes(`@${target.name}`) ? text : `@${target.name} ${text}`
    const message = await this.rooms.append(room.id, {
      author: { kind: 'bot', botId: from.id },
      text: body,
      mentions: [target.id],
      ...(turn?.chain ? { chain: turn.chain } : {}),
    })
    // 没叫醒对方（客气话、链到上限）要如实告诉这个 Bot，不然它会一直等回答。
    const result = await this.wakeFromBot(room, from, target.id, message, turn?.chain || null)
    return { type: 'message', bot: target.id, room: room.id, ...result }
  },

  // 席位会话收到一句不在群记录里的话（问题卡片的回答、到点的定时）：当成叫醒送进去。
  deliverSeatNote(sessionId, text, { kind, trust }) {
    const seat = this.roomSeatOf(sessionId)
    if (!seat) return null
    return this.wakeSeat(seat.roomId, seat.botId, {
      note: text,
      reason: { kind },
      trust,
    })
  },

  async recallRoomMessage(id, messageId) {
    this.requireRoom(id)
    const message = await this.rooms.message(id, String(messageId || ''))
    if (!message) throw pisperError('not_found', 'message')
    if (message.author?.kind === 'system') throw pisperError('forbidden', 'message')
    if (message.turn?.state === 'running')
      throw pisperError('busy', 'message', 'the bot is still writing it; stop it first')
    const state = await this.rooms.state(id)
    const authorBot = message.author?.kind === 'bot' ? message.author.botId : ''
    const seenBy = Object.entries(state.seats)
      .filter(([botId, seat]) => botId !== authorBot && seat.readSeq >= message.seq)
      .map(([botId]) => botId)
    const placeholder = await this.rooms.recall(id, message.id, { seenBy })
    if (seenBy.length) await this.rooms.noteRecall(id, seenBy, message.seq)
    // 记录里的附件引用随占位一起擦掉了；只有这条消息用到的上传文件也删掉。
    // Bot 已经收到过的（席位会话里有引用）留着：它的会话记录里本来就还有。
    const uploaded = unique(
      (message.attachments || []).map((ref) => ref?.id).filter((item) => typeof item === 'string'),
    ).length
    const deleted = await this.releaseRoomAttachments(message.attachments).catch(() => {
      console.warn('[rooms] Could not delete the recalled attachments.')
      return 0
    })
    return {
      message: placeholder,
      seenBy,
      // 还留在资产库里的上传文件有几个：如实告诉你，撤回擦掉的是群记录，不是 Bot 已经拿到的那份。
      attachmentsKept: Math.max(0, uploaded - deleted),
      briefMayContain: Boolean(state.brief && state.brief.upToSeq >= message.seq),
    }
  },

  // 工作卡上的批准 / 拒绝：交给这张卡所在的席位会话。
  async resolveRoomApproval(id, messageId, { approvalId, approve }) {
    this.requireRoom(id)
    const message = await this.rooms.message(id, String(messageId || ''))
    const sessionId = message?.turn?.sessionId
    if (!sessionId) throw pisperError('not_found', 'message')
    const resolution = this.resolveToolApproval(
      sessionId,
      String(approvalId || ''),
      approve === true,
    )
    if (!resolution?.found) throw pisperError('not_found', 'request', 'already handled')
    return { resolved: true }
  },

  // 停下一张工作卡（停掉那个席位会话的这一轮）。
  async stopRoomTurn(id, messageId) {
    this.requireRoom(id)
    const message = await this.rooms.message(id, String(messageId || ''))
    const sessionId = message?.turn?.sessionId
    if (!sessionId) throw pisperError('not_found', 'message')
    // 只停这张卡自己的那一轮：点的是旧卡（它早结束了）时，不能把这个席位正在跑的新一轮停掉。
    if (this.roomSeatTurns?.get(sessionId)?.messageId !== message.id) return { stopped: false }
    await this.abortSession(sessionId)
    return { stopped: true }
  },

  // 整理简报：维护的 Bot 用它的模型做一次不带工具的独立请求（不进席位会话、不动任何文件），只回 JSON。
  async refreshRoomBrief(id, { botId = '', auto = false } = {}) {
    const room = this.requireRoom(id)
    this.roomBriefRuns ||= new Map()
    if (this.roomBriefRuns.has(id)) return this.roomBriefRuns.get(id)
    const maintainer = String(botId || room.briefBotId || '')
    if (!maintainer || !room.members.includes(maintainer)) {
      if (auto) return null
      throw pisperError('invalid', 'bot', 'pick a member bot to keep the brief')
    }
    const bot = this.bots.get(maintainer)
    if (!bot) throw pisperError('not_found', 'bot')
    const run = (async () => {
      const [messages, state] = await Promise.all([this.rooms.messages(id), this.rooms.state(id)])
      const members = this.roomMembers(room).filter((member) => member.name)
      const name = (author) =>
        author?.kind === 'bot'
          ? members.find((member) => member.id === author.botId)?.name || 'a removed bot'
          : 'user'
      const from = state.brief?.upToSeq || 0
      // 只整理到第一张还在做的卡之前：它的结果还没写上，越过去就再也不会被整理进来。
      const unfinished = messages.find(
        (message) => message.seq > from && message.turn?.state === 'running',
      )
      const cover = unfinished ? unfinished.seq - 1 : messages.at(-1)?.seq || from
      const lines = []
      let size = 0
      const fresh = messages.filter(
        (message) =>
          message.seq > from &&
          message.seq <= cover &&
          !message.recalled &&
          message.text &&
          message.author?.kind !== 'system',
      )
      for (let index = fresh.length - 1; index >= 0; index -= 1) {
        const message = fresh[index]
        const line = `#${message.seq} ${name(message.author)}: ${oneLine(message.text, 1_200)}`
        if (size + line.length > BRIEF_SOURCE_CHARS) break
        lines.unshift(line)
        size += line.length
      }
      if (!lines.length) return state.brief
      const model = await this.roomBriefModel(id, bot)
      if (!model) throw pisperError('unavailable', 'model')
      const previous = state.brief
        ? { decisions: state.brief.decisions, todos: state.brief.todos }
        : null
      const result = await requestBotText({
        modelRuntime: this.modelRuntime,
        model,
        text: roomBriefInstruction({ room, previous, members, lines }),
      })
      if (result.usage)
        await this.recordUsage?.(
          localDayKey(),
          `room-brief:${id}:${randomUUID()}`,
          result.usage,
        ).catch(() => {})
      const parsed = result.text === null ? null : parseBriefReply(result.text, room.members)
      if (!parsed) throw pisperError('failed', 'operation', 'the bot did not return a brief')
      const brief = {
        ...parsed,
        upToSeq: cover,
        by: maintainer,
        at: new Date().toISOString(),
      }
      await this.rooms.setBrief(id, brief)
      return brief
    })()
    this.roomBriefRuns.set(id, run)
    // 右栏的“正在整理”跟着变。
    this.roomStoreChanged(id, ['detail'])
    try {
      return await run
    } finally {
      this.roomBriefRuns.delete(id)
      this.roomStoreChanged(id, ['detail'])
    }
  },

  // 整理简报用哪个模型：和复盘一样，先用这个 Bot 正在用的（它在这个群里的席位、它的长期对话），
  // 再按它的设定挑；都没有时打开它的会话取会话模型（Bot 的“档位”没配时就是跟着默认模型走）。
  async roomBriefModel(roomId, bot) {
    const seat = await this.rooms.seat(roomId, bot.id)
    const sessionIds = [seat?.sessionId, bot.threadSessionId].filter(Boolean)
    for (const sessionId of sessionIds) {
      const model = this.sessions.get(sessionId)?.session?.model
      if (model) return model
    }
    const resolved = await this.providerPreferences.resolveSubagentModel(
      bot.model.ref ? { modelRef: bot.model.ref } : { tier: bot.model.tier },
    )
    if (resolved.model) return resolved.model
    const sessionId = sessionIds[0] || (await this.ensureBotSession(bot.id))
    return (await this.getOrCreateSession(sessionId))?.session?.model || null
  },

  // 你手改简报：覆盖到当前最后一条，作者记为你。
  async editRoomBrief(id, input) {
    const room = this.requireRoom(id)
    const brief = normalizeBrief(input, room.members)
    return this.rooms.setBrief(id, {
      ...brief,
      upToSeq: await this.rooms.lastSeq(id),
      by: 'user',
      at: new Date().toISOString(),
    })
  },

  // 发消息带的附件归档成资产，消息里只留引用。图片要模型能看图才收（同会话输入框）。
  async storeRoomAttachments(room, attachments) {
    if (!attachments.length) return []
    if (
      attachments.some((attachment) => attachment.kind === 'image') &&
      this.capabilities?.features?.imageProcessing === false
    )
      throw pisperError('unsupported', 'image', 'image attachments')
    return archiveRoomAttachments({
      attachments,
      roomName: room.name,
      // 归档到写进群记录之间，这份资产“还没有哪条消息引用”：记为正在用，免得同时进行的撤回
      //（撤回的那条带着同一个文件）把它当成没人要的删掉。
      createAsset: async (input) => {
        const asset = await this.createAsset(input)
        if (asset?.id) this.holdRoomAsset(asset.id, 1)
        return asset
      },
      release: (assetId) => {
        this.holdRoomAsset(assetId, -1)
        return this.releaseRoomAttachments([{ id: assetId }])
      },
    })
  },

  holdRoomAsset(assetId, delta) {
    this.roomAssetHolds ||= new Map()
    const count = (this.roomAssetHolds.get(assetId) || 0) + delta
    if (count > 0) this.roomAssetHolds.set(assetId, count)
    else this.roomAssetHolds.delete(assetId)
  },

  // 同一条消息叫醒好几个 Bot 时共用一份附件内容；最后一次用到的几秒后放掉，不长期占着内存。
  sharedRoomAttachmentReads() {
    this.roomAttachmentReads ||= new Map()
    if (this.roomAttachmentReadsTimer) clearTimeout(this.roomAttachmentReadsTimer)
    this.roomAttachmentReadsTimer = setTimeout(() => {
      this.roomAttachmentReadsTimer = null
      this.roomAttachmentReads.clear()
    }, ROOM_ATTACHMENT_SHARE_MS)
    this.roomAttachmentReadsTimer.unref?.()
    return this.roomAttachmentReads
  },

  // 删掉只被群消息用过、现在也没有哪条没撤回的群消息还引用着的资产；别处用到的一律不动。
  async releaseRoomAttachments(refs) {
    const ids = unique(
      (Array.isArray(refs) ? refs : [])
        .map((ref) => ref?.id)
        .filter((id) => typeof id === 'string'),
    )
    if (!ids.length || !this.findAsset || !this.deleteAsset) return 0
    const lists = await Promise.all(
      this.rooms.list().map((room) => this.rooms.peekMessages(room.id).catch(() => [])),
    )
    let deleted = 0
    for (const assetId of ids) {
      if (this.roomAssetHolds?.has(assetId)) continue
      if (!isRoomOnlyAsset(this.findAsset(assetId)) || roomsStillUse(lists, assetId)) continue
      if (await this.deleteAsset(assetId)) deleted += 1
    }
    return deleted
  },

  // 删 Bot 时：从所有群里移出。
  async removeBotFromRooms(botId) {
    if (!this.rooms) return
    // 只剩它一个成员的群会跟着删掉：群里上传、别处没用到的附件一并清掉（同 deleteRoom）。
    const refs = []
    for (const room of this.rooms.list())
      if (room.members.length === 1 && room.members[0] === botId)
        refs.push(
          ...(await this.rooms.peekMessages(room.id).catch(() => [])).flatMap(
            (message) => message.attachments || [],
          ),
        )
    await this.rooms.removeBot(botId)
    if (refs.length) await this.releaseRoomAttachments(refs).catch(() => {})
  },
}
