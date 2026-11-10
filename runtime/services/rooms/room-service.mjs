// RoomService：Bot 群聊的唯一权威。群列表在 pisper-rooms.json（启动体检覆盖），每个群一个目录：
// messages.jsonl 平时只追加（新消息、对已有消息的补丁行），撤回时原子重写整个文件、把那条换成占位，
// 原文不留在磁盘上；state.json 记每个 Bot 的席位（席位会话、读到第几条、还没告知的撤回）和简报。
// 群目录里的文件坏了只影响这个群：挪成 .corrupt-<时间>，从空的开始，其余照常。
import { appendFile, mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { pisperError } from '../../../shared/errors.mjs'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'
import { setAsideDataFile } from '../startup-data-check.mjs'
import {
  MAX_ROOMS,
  MAX_ROOM_STORED_CHARS,
  ROOM_ID_PATTERN,
  newRoomId,
  newRoomMessageId,
  normalizeBrief,
  normalizeRoomInput,
} from './room-model.mjs'

const STORE_VERSION = 1
const FILE_MODE = 0o600
// 补丁行攒到这么多，下次读盘时把文件重写一遍合并掉。
const PATCH_COMPACT_LINES = 500

function clone(value) {
  return value === undefined ? undefined : JSON.parse(JSON.stringify(value))
}

function emptyState() {
  return { seats: {}, brief: null }
}

// state.json 读进来逐项归一：坏的席位丢掉，不让一条坏数据拖垮整个群。
function normalizeState(value, members) {
  const state = emptyState()
  const seats = value && typeof value.seats === 'object' && value.seats ? value.seats : {}
  for (const [botId, seat] of Object.entries(seats)) {
    if (!/^bot_[A-Za-z0-9_-]{6,40}$/.test(botId) || !seat || typeof seat !== 'object') continue
    state.seats[botId] = {
      sessionId: typeof seat.sessionId === 'string' ? seat.sessionId.slice(0, 200) : '',
      readSeq: Number.isSafeInteger(seat.readSeq) && seat.readSeq > 0 ? seat.readSeq : 0,
      lastWakeAt: typeof seat.lastWakeAt === 'string' ? seat.lastWakeAt.slice(0, 40) : '',
      recalls: Array.isArray(seat.recalls)
        ? seat.recalls.filter((seq) => Number.isSafeInteger(seq)).slice(-50)
        : [],
      late: Array.isArray(seat.late)
        ? seat.late.filter((seq) => Number.isSafeInteger(seq)).slice(-20)
        : [],
    }
  }
  const brief = value?.brief
  if (brief && typeof brief === 'object') {
    state.brief = {
      ...normalizeBrief(brief, members),
      upToSeq: Number.isSafeInteger(brief.upToSeq) ? brief.upToSeq : 0,
      by: typeof brief.by === 'string' ? brief.by.slice(0, 64) : '',
      at: typeof brief.at === 'string' ? brief.at.slice(0, 40) : '',
    }
  }
  return state
}

// 一行 JSONL → 消息或补丁；认不出的行跳过。
function parseLine(line) {
  try {
    const value = JSON.parse(line)
    return value && typeof value === 'object' ? value : null
  } catch {
    return null
  }
}

export class RoomService {
  constructor({ dataDir, now = () => Date.now() }) {
    this.path = join(dataDir, 'pisper-rooms.json')
    this.dir = join(dataDir, 'rooms')
    this.now = now
    this.rooms = []
    // roomId → { messages, state, queue }，第一次用到时读盘。
    this.logs = new Map()
    // 席位会话 → { roomId, botId }：运行时据此认出“这是某个 Bot 在某个群里的会话”。
    this.seatSessions = new Map()
    this.writeQueue = Promise.resolve()
  }

  roomDir(id) {
    if (!ROOM_ID_PATTERN.test(id)) throw pisperError('not_found', 'room')
    return join(this.dir, id)
  }

  async init() {
    const stored = await readJson(this.path, { version: STORE_VERSION, rooms: [] })
    const rooms = []
    for (const value of Array.isArray(stored?.rooms) ? stored.rooms.slice(0, MAX_ROOMS) : []) {
      try {
        if (typeof value?.id !== 'string' || !ROOM_ID_PATTERN.test(value.id)) continue
        rooms.push({
          id: value.id,
          ...normalizeRoomInput(value),
          createdAt: String(value.createdAt || ''),
          updatedAt: String(value.updatedAt || ''),
        })
      } catch {
        // 跳过这一条，其余照常加载。
      }
    }
    this.rooms = rooms
    // 席位索引要在启动时就有：Runtime 重启后第一轮跑到席位会话时，运行时得知道它属于谁。
    for (const room of rooms) await this.load(room.id).catch(() => null)
  }

  save() {
    const snapshot = { version: STORE_VERSION, rooms: clone(this.rooms) }
    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(() => writeJsonAtomic(this.path, snapshot, { mode: FILE_MODE }))
    return this.writeQueue
  }

  list() {
    return clone(this.rooms)
  }

  get(id) {
    const room = this.rooms.find((item) => item.id === id)
    return room ? clone(room) : null
  }

  require(id) {
    const room = this.get(id)
    if (!room) throw pisperError('not_found', 'room')
    return room
  }

  async create(input) {
    if (this.rooms.length >= MAX_ROOMS)
      throw pisperError('limit', 'room', `max ${MAX_ROOMS}`, { status: 409 })
    const at = new Date(this.now()).toISOString()
    const room = { id: newRoomId(), ...normalizeRoomInput(input), createdAt: at, updatedAt: at }
    this.rooms.push(room)
    this.logs.set(room.id, { messages: [], state: emptyState(), queue: Promise.resolve() })
    await mkdir(this.roomDir(room.id), { recursive: true, mode: 0o700 })
    await this.save()
    return clone(room)
  }

  async update(id, input) {
    const index = this.rooms.findIndex((item) => item.id === id)
    if (index < 0) return null
    const current = this.rooms[index]
    const updated = {
      id,
      ...normalizeRoomInput(input, current),
      createdAt: current.createdAt,
      updatedAt: new Date(this.now()).toISOString(),
    }
    this.rooms[index] = updated
    await this.save()
    return clone(updated)
  }

  // 删群：记录与状态一起删掉；席位会话留着，变成普通会话（同删 Bot）。
  async remove(id) {
    const index = this.rooms.findIndex((item) => item.id === id)
    if (index < 0) return false
    this.rooms.splice(index, 1)
    for (const [sessionId, seat] of this.seatSessions)
      if (seat.roomId === id) this.seatSessions.delete(sessionId)
    this.logs.delete(id)
    await this.save()
    await rm(this.roomDir(id), { recursive: true, force: true })
    return true
  }

  // 删 Bot：从所有群里移出；它说过的话留着（界面显示“已删除的 Bot”）。只剩它一个成员的群一并删掉。
  async removeBot(botId) {
    for (const room of [...this.rooms]) {
      if (!room.members.includes(botId)) continue
      const members = room.members.filter((id) => id !== botId)
      if (!members.length) await this.remove(room.id)
      else await this.update(room.id, { members })
    }
    for (const [sessionId, seat] of this.seatSessions)
      if (seat.botId === botId) this.seatSessions.delete(sessionId)
  }

  // 读一个群的记录与状态（只读一次，之后都在内存里）。
  async load(id) {
    const existing = this.logs.get(id)
    if (existing) return existing
    const room = this.require(id)
    const dir = this.roomDir(id)
    const messages = []
    const byId = new Map()
    const text = await readFile(join(dir, 'messages.jsonl'), 'utf8').catch((error) => {
      if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return ''
      throw error
    })
    let patches = 0
    for (const line of text.split('\n')) {
      if (!line.trim()) continue
      const value = parseLine(line)
      if (!value) continue
      if (value.op === 'patch') {
        patches += 1
        const target = byId.get(value.id)
        if (target && value.set && typeof value.set === 'object') Object.assign(target, value.set)
        continue
      }
      if (!Number.isSafeInteger(value.seq) || typeof value.id !== 'string') continue
      if (byId.has(value.id)) continue
      messages.push(value)
      byId.set(value.id, value)
    }
    messages.sort((left, right) => left.seq - right.seq)
    // 读盘只发生在进程第一次用到这个群时，这时不可能有哪一轮真在跑：盘上还写着“在做”的卡是上次退出时
    // 留下的，收成“已停下”。不收的话它永远转圈、撤不回，也永远进不了别的 Bot 的未读。
    let stale = 0
    for (const message of messages) {
      if (message.turn?.state !== 'running') continue
      stale += 1
      message.turn = {
        ...message.turn,
        state: 'stopped',
        interrupted: true,
        endedAt: new Date(this.now()).toISOString(),
      }
    }
    let state = emptyState()
    const statePath = join(dir, 'state.json')
    try {
      state = normalizeState(await readJson(statePath, emptyState()), room.members)
    } catch {
      // 读不了：留底后从空的开始（席位会话还在，只是这个群不再认得它们）。
      await setAsideDataFile(statePath).catch(() => '')
    }
    const log = { messages, state, queue: Promise.resolve() }
    // 并发的第一次读：以先放进去的为准。
    if (this.logs.has(id)) return this.logs.get(id)
    this.logs.set(id, log)
    if (stale || patches > PATCH_COMPACT_LINES)
      void this.#rewrite(id, log).catch(() => console.warn('[rooms] Could not tidy the room log.'))
    for (const [botId, seat] of Object.entries(state.seats))
      if (seat.sessionId) this.seatSessions.set(seat.sessionId, { roomId: id, botId })
    return log
  }

  // 同一个群的写入按顺序来（追加、补丁、重写、状态）。
  #enqueue(log, task) {
    const next = log.queue.catch(() => {}).then(task)
    log.queue = next
    return next
  }

  // 把内存里的记录原子地重写到盘上（补丁行合并掉）。内容在排队时就定下，之后排进来的追加照常接在后面。
  #rewrite(id, log) {
    const path = join(this.roomDir(id), 'messages.jsonl')
    const body = log.messages.map((item) => JSON.stringify(item)).join('\n')
    return this.#enqueue(log, async () => {
      const temporary = `${path}.${process.pid}.${Date.now()}.tmp`
      await writeFile(temporary, body ? `${body}\n` : '', { encoding: 'utf8', mode: FILE_MODE })
      await rename(temporary, path)
    })
  }

  async messages(id) {
    return clone((await this.load(id)).messages)
  }

  // 只读视图：不拷贝，调用方不得改动（轮询每一两秒读一次，整份深拷贝太贵）。
  async peekMessages(id) {
    return (await this.load(id)).messages
  }

  async message(id, messageId) {
    const log = await this.load(id)
    return clone(log.messages.find((item) => item.id === messageId) || null)
  }

  async lastSeq(id) {
    const log = await this.load(id)
    return log.messages.at(-1)?.seq || 0
  }

  /**
   * 追加一条消息，返回带 seq 与 id 的完整消息。
   * @param {string} id
   * @param {Record<string, any>} input
   */
  async append(id, input) {
    const log = await this.load(id)
    const dir = this.roomDir(id)
    const message = {
      ...input,
      ...(typeof input.text === 'string'
        ? { text: input.text.slice(0, MAX_ROOM_STORED_CHARS) }
        : {}),
      seq: (log.messages.at(-1)?.seq || 0) + 1,
      id: newRoomMessageId(),
      at: new Date(this.now()).toISOString(),
    }
    log.messages.push(message)
    await this.#enqueue(log, async () => {
      await mkdir(dir, { recursive: true, mode: 0o700 })
      await appendFile(join(dir, 'messages.jsonl'), `${JSON.stringify(message)}\n`, {
        encoding: 'utf8',
        mode: FILE_MODE,
      })
    })
    return clone(message)
  }

  // 改一条已有消息的几个字段（工作卡的状态、回复正文）：内存里直接改，盘上追加一行补丁。
  async patch(id, messageId, set) {
    const log = await this.load(id)
    const message = log.messages.find((item) => item.id === messageId)
    if (!message) return null
    const clean = { ...set }
    if (typeof clean.text === 'string') clean.text = clean.text.slice(0, MAX_ROOM_STORED_CHARS)
    Object.assign(message, clean)
    await this.#enqueue(log, () =>
      appendFile(
        join(this.roomDir(id), 'messages.jsonl'),
        `${JSON.stringify({ op: 'patch', id: messageId, set: clean })}\n`,
        { encoding: 'utf8', mode: FILE_MODE },
      ),
    )
    return clone(message)
  }

  // 撤回：正文、提及、引用与回复一起擦掉，只留序号、作者、时间和“谁读到过”；整个文件原子重写，
  // 补丁行顺便合并掉，原文不留在磁盘上。
  async recall(id, messageId, { seenBy = [] } = {}) {
    const log = await this.load(id)
    const index = log.messages.findIndex((item) => item.id === messageId)
    if (index < 0) return null
    const current = log.messages[index]
    if (current.recalled) return clone(current)
    const placeholder = {
      seq: current.seq,
      id: current.id,
      at: current.at,
      author: current.author,
      recalled: { at: new Date(this.now()).toISOString(), seenBy },
    }
    log.messages[index] = placeholder
    await this.#rewrite(id, log)
    return clone(placeholder)
  }

  async state(id) {
    return clone((await this.load(id)).state)
  }

  #saveState(id, log) {
    const snapshot = clone(log.state)
    return this.#enqueue(log, () =>
      writeJsonAtomic(join(this.roomDir(id), 'state.json'), snapshot, { mode: FILE_MODE }),
    )
  }

  async seat(id, botId) {
    const log = await this.load(id)
    return clone(log.state.seats[botId] || null)
  }

  // 改某个 Bot 的席位（会话、读到第几条、还没告知的撤回）。
  async setSeat(id, botId, patch) {
    const log = await this.load(id)
    const seat = {
      sessionId: '',
      readSeq: 0,
      lastWakeAt: '',
      recalls: [],
      late: [],
      ...(log.state.seats[botId] || {}),
      ...patch,
    }
    // “读到第几条”只进不退：晚到的写入不能把它拉回去（拉回去就会把同一段再送一遍）。
    seat.readSeq = Math.max(seat.readSeq || 0, log.state.seats[botId]?.readSeq || 0)
    log.state.seats[botId] = seat
    if (seat.sessionId) this.seatSessions.set(seat.sessionId, { roomId: id, botId })
    await this.#saveState(id, log)
    return clone(seat)
  }

  // 撤回一条别人已经读到过的消息：记到它的席位上，下次叫醒时告诉它“第 N 条撤回了，别再引用”。
  async noteRecall(id, botIds, seq) {
    const log = await this.load(id)
    for (const botId of botIds) {
      const seat = log.state.seats[botId]
      if (seat && !seat.recalls.includes(seq)) seat.recalls = [...seat.recalls, seq].slice(-50)
    }
    await this.#saveState(id, log)
  }

  async setBrief(id, brief) {
    const log = await this.load(id)
    log.state.brief = brief ? clone(brief) : null
    await this.#saveState(id, log)
    return clone(log.state.brief)
  }

  seatOfSession(sessionId) {
    const seat = this.seatSessions.get(String(sessionId || ''))
    return seat ? { ...seat } : null
  }

  // 这个群里各个 Bot 的席位会话。
  sessionsOfRoom(roomId) {
    return [...this.seatSessions.entries()]
      .filter(([, seat]) => seat.roomId === roomId)
      .map(([sessionId, seat]) => ({ sessionId, botId: seat.botId }))
  }

  // 这个 Bot 在各个群里的席位会话。
  sessionsOfBot(botId) {
    return [...this.seatSessions.entries()]
      .filter(([, seat]) => seat.botId === botId)
      .map(([sessionId, seat]) => ({ sessionId, roomId: seat.roomId }))
  }
}
