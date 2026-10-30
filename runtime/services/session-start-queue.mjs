// 会话启动排队（规格 8.4 "排队 startAfter"，能力 sessionQueue）：派发对话框或主页草稿选"排在 X 之后"时，
// 新会话不立刻运行，而是登记 { sessionId, startAfter: X, message }；X 这一轮结束（完成、失败、停止、
// 被删除都算）时由这里启动它，"立即开始"可以跳过等待。不做"同一工作区自动串行"（规格第 11 节第 5 问）。
// 队列持久化在 <dataDir>/pisper-session-queue.json（原子写入、按文件串行），Runtime 重启后恢复：
// 前置会话此时都已不在运行（它的那一轮随进程结束了），等启动就绪后按顺序启动等它的会话。
import { rename } from 'node:fs/promises'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

export const SESSION_QUEUE_LIMITS = Object.freeze({ entries: 200, message: 100_000 })
const RUN_MODES = new Set(['plan', 'goal', 'team'])

export class SessionQueueError extends Error {
  constructor(code, message, status = 400) {
    super(message)
    this.name = 'SessionQueueError'
    this.code = code
    this.status = status
  }
}

function normalizeEntry(raw) {
  if (!raw || typeof raw !== 'object') return null
  const sessionId = typeof raw.sessionId === 'string' ? raw.sessionId : ''
  const startAfter = typeof raw.startAfter === 'string' ? raw.startAfter : ''
  const message = typeof raw.message === 'string' ? raw.message : ''
  const createdAt =
    typeof raw.createdAt === 'string' && Number.isFinite(Date.parse(raw.createdAt))
      ? raw.createdAt
      : ''
  if (!sessionId || !startAfter || sessionId === startAfter || !message.trim() || !createdAt)
    return null
  return {
    sessionId,
    startAfter,
    message: message.slice(0, SESSION_QUEUE_LIMITS.message),
    runMode: RUN_MODES.has(raw.runMode) ? raw.runMode : 'plan',
    createdAt,
  }
}

export class SessionStartQueue {
  /**
   * @param {{
   *   path?: string,
   *   startSession: (entry: { sessionId: string, message: string, runMode: string }) => Promise<unknown> | unknown,
   *   isRunning: (sessionId: string) => boolean,
   *   sessionExists: (sessionId: string) => Promise<boolean> | boolean,
   *   onChange?: (sessionId: string) => void,
   *   onError?: (error: unknown) => void,
   * }} options
   */
  constructor({ path, startSession, isRunning, sessionExists, onChange, onError }) {
    this.path = path || ''
    this.startSession = startSession
    this.isRunning = isRunning
    this.sessionExists = sessionExists
    this.onChange = typeof onChange === 'function' ? onChange : () => {}
    this.onError = typeof onError === 'function' ? onError : () => {}
    /** @type {Map<string, NonNullable<ReturnType<typeof normalizeEntry>>>} */
    this.entries = new Map()
    this.write = Promise.resolve()
    this.loaded = false
  }

  async load() {
    if (this.loaded) return
    this.loaded = true
    if (!this.path) return
    let stored
    try {
      stored = await readJson(this.path, { version: 1, entries: [] })
    } catch {
      // 文件损坏：先改名留底（.corrupt-<时间>），再从空队列开始，不静默覆盖用户数据。
      await rename(this.path, `${this.path}.corrupt-${Date.now()}`).catch(() => {})
      stored = { version: 1, entries: [] }
    }
    const entries = Array.isArray(stored?.entries) ? stored.entries : []
    for (const raw of entries.slice(0, SESSION_QUEUE_LIMITS.entries)) {
      const entry = normalizeEntry(raw)
      if (entry && !this.entries.has(entry.sessionId)) this.entries.set(entry.sessionId, entry)
    }
  }

  // 每次变更都排进同一条写入链：writeJsonAtomic 的临时文件名同一毫秒内会撞名。
  persist() {
    if (!this.path) return Promise.resolve()
    const snapshot = { version: 1, entries: this.list() }
    this.write = this.write.catch(() => {}).then(() => writeJsonAtomic(this.path, snapshot))
    return this.write
  }

  list() {
    return [...this.entries.values()].sort((left, right) =>
      left.createdAt === right.createdAt
        ? left.sessionId.localeCompare(right.sessionId)
        : left.createdAt.localeCompare(right.createdAt),
    )
  }

  /** 会话摘要与看板卡片里的 queue 字段：{ position（全局第几位，从 1 起）, startAfter, createdAt }。 */
  summary(sessionId) {
    const entry = this.entries.get(sessionId)
    if (!entry) return null
    const position = this.list().findIndex((item) => item.sessionId === sessionId) + 1
    return { position, startAfter: entry.startAfter, createdAt: entry.createdAt }
  }

  // X 是否还会"结束一次"：在运行，或者自己也在排队（排队链 A → B → C）。
  pending(sessionId) {
    return this.isRunning(sessionId) || this.entries.has(sessionId)
  }

  // startAfter 链能回到自己就是环：A 等 B、B 等 A 永远不会启动。
  createsCycle(sessionId, startAfter) {
    const seen = new Set([sessionId])
    let cursor = startAfter
    while (cursor) {
      if (seen.has(cursor)) return true
      seen.add(cursor)
      cursor = this.entries.get(cursor)?.startAfter || ''
    }
    return false
  }

  /**
   * 登记"排在 startAfter 之后"。前置会话已经不会再结束一次（没在运行也没在排队）时直接启动。
   * @returns {Promise<{ queued: true, queue: NonNullable<ReturnType<SessionStartQueue['summary']>> } | { queued: false, started: true }>}
   */
  async enqueue({ sessionId, startAfter, message, runMode }) {
    await this.load()
    const id = typeof sessionId === 'string' ? sessionId : ''
    const after = typeof startAfter === 'string' ? startAfter.trim() : ''
    const text = typeof message === 'string' ? message.trim() : ''
    if (!after) throw new SessionQueueError('start_after_required', '请选择要排在哪个会话之后。')
    if (after === id) throw new SessionQueueError('start_after_self', '不能排在自己之后。')
    if (!text) throw new SessionQueueError('message_required', '排队的消息不能为空。')
    if (Array.from(text).length > SESSION_QUEUE_LIMITS.message)
      throw new SessionQueueError('message_too_long', '排队的消息太长。')
    if (!(await this.sessionExists(id)))
      throw new SessionQueueError('session_not_found', '会话不存在。', 404)
    if (!(await this.sessionExists(after)))
      throw new SessionQueueError('start_after_not_found', '要排在其后的会话不存在。', 404)
    if (this.isRunning(id))
      throw new SessionQueueError('session_running', '会话正在运行，不能再排队。', 409)
    if (this.entries.has(id)) throw new SessionQueueError('already_queued', '会话已经在排队。', 409)
    if (this.entries.size >= SESSION_QUEUE_LIMITS.entries)
      throw new SessionQueueError('queue_full', '排队的会话太多了。', 409)
    if (this.createsCycle(id, after))
      throw new SessionQueueError('start_after_cycle', '排队顺序形成了循环。')
    const entry = {
      sessionId: id,
      startAfter: after,
      message: text,
      runMode: RUN_MODES.has(runMode) ? runMode : 'plan',
      createdAt: new Date().toISOString(),
    }
    if (!this.pending(after)) {
      // 前置会话已经结束：等它"结束"永远等不到，直接启动。
      this.launch(entry)
      return { queued: false, started: true }
    }
    this.entries.set(id, entry)
    await this.persist()
    this.onChange(id)
    return { queued: true, queue: this.summary(id) }
  }

  /** 立即开始：跳过等待。 */
  async startNow(sessionId) {
    await this.load()
    const entry = this.entries.get(sessionId)
    if (!entry) throw new SessionQueueError('not_queued', '会话不在排队中。', 404)
    this.entries.delete(sessionId)
    await this.persist()
    this.onChange(sessionId)
    this.launch(entry)
    return { started: true }
  }

  /** 取消排队：会话留下（没有消息），返回原消息供客户端放回草稿。 */
  async cancel(sessionId) {
    await this.load()
    const entry = this.entries.get(sessionId)
    if (!entry) throw new SessionQueueError('not_queued', '会话不在排队中。', 404)
    this.entries.delete(sessionId)
    await this.persist()
    this.onChange(sessionId)
    return { cancelled: true, message: entry.message }
  }

  /** 某个会话的这一轮结束（完成、失败、停止都算）：启动排在它后面的会话。 */
  async onRunFinished(sessionId) {
    await this.load()
    const ready = this.list().filter((entry) => entry.startAfter === sessionId)
    if (!ready.length) return 0
    for (const entry of ready) this.entries.delete(entry.sessionId)
    await this.persist()
    for (const entry of ready) {
      this.onChange(entry.sessionId)
      this.launch(entry)
    }
    return ready.length
  }

  /** 会话被删除：它自己的排队作废；排在它后面的会话视为前置已结束，照常启动。 */
  async onSessionDeleted(sessionId) {
    await this.load()
    if (this.entries.delete(sessionId)) {
      await this.persist()
      this.onChange(sessionId)
    }
    return this.onRunFinished(sessionId)
  }

  /** 启动恢复：前置会话已不会再结束一次的排队项（例如上次进程退出时它在运行）按顺序启动。 */
  async resume() {
    await this.load()
    const ready = this.list().filter((entry) => !this.pending(entry.startAfter))
    if (!ready.length) return 0
    for (const entry of ready) this.entries.delete(entry.sessionId)
    await this.persist()
    for (const entry of ready) {
      this.onChange(entry.sessionId)
      this.launch(entry)
    }
    return ready.length
  }

  // 后台启动：不等运行结束；失败只记一条不含用户内容的日志，排队项已出队，不会反复重试。
  launch(entry) {
    void Promise.resolve()
      .then(() =>
        this.startSession({
          sessionId: entry.sessionId,
          message: entry.message,
          runMode: entry.runMode,
        }),
      )
      .catch((error) => this.onError(error))
  }

  async dispose() {
    await this.write.catch(() => {})
  }
}
