// 看板事件流（GET /api/board/events）的服务端集线器：所有连接共用一份修订号、增量缓冲与节流。
// - 修订号从创建时的毫秒时间起单调递增：Runtime 重启后旧客户端带来的 after 必然小于新起点，
//   落进“缺口”分支收到 resync_required，而不会误把新进程的增量当成补发。
// - 会话变化只记“脏”标记（O(1)），由定时 flush 统一读一次会话列表再生成卡片增量；
//   每个会话两次增量至少间隔 250ms（每秒最多 4 次），尾巴等高频变化在窗口内自然合并。
// - 没有连接时不跑任何定时器，只累计脏会话 id（有上限）；下一次连接时先补算再发快照或补发。
// 传输细节（SSE 写出、序列化）由路由层通过连接的 send/close 回调提供，这里不依赖 HTTP 模块。

/**
 * @typedef {{ id: string, session: Record<string, unknown>, card: Record<string, unknown> } | null} BoardHubItem
 * @typedef {{
 *   listItems: (ids: string[] | null) => Promise<Map<string, BoardHubItem>>,
 *   snapshotLimit?: number,
 *   minIntervalMs?: number,
 *   coalesceMs?: number,
 *   minFlushGapMs?: number,
 *   heartbeatMs?: number,
 *   maxBufferedEvents?: number,
 *   maxBufferedBytes?: number,
 *   maxDirty?: number,
 *   maxConnections?: number,
 *   maxQueuedPerConnection?: number,
 *   now?: () => number,
 *   onError?: (error: unknown) => void,
 * }} BoardEventHubOptions
 * @typedef {{ revision: number, event: string, data: Record<string, unknown>, bytes: number, id: string }} BufferedEvent
 * @typedef {{
 *   send: (event: string, data: Record<string, unknown>) => boolean | void,
 *   close: () => void,
 *   ready: boolean,
 *   queued: BufferedEvent[],
 *   closed: boolean,
 * }} BoardConnection
 */

export const BOARD_EVENTS_MAX_CONNECTIONS = 32
export const BOARD_CARD_MIN_INTERVAL_MS = 250
export const BOARD_HEARTBEAT_MS = 15_000
const DEFAULT_COALESCE_MS = 50
// 两次 flush 之间至少间隔 100ms：多个会话各自的节流窗口错开时，整体每秒最多读 10 次会话列表。
const DEFAULT_MIN_FLUSH_GAP_MS = 100
const DEFAULT_MAX_BUFFERED_EVENTS = 512
const DEFAULT_MAX_BUFFERED_BYTES = 2 * 1024 * 1024
const DEFAULT_MAX_DIRTY = 4096
const DEFAULT_MAX_QUEUED_PER_CONNECTION = 1024
// 记住最近判定过“不上看板”的会话，避免它们每次变化都重复发 removed。
const MAX_ELIGIBILITY_ENTRIES = 4096
// component_changed 的 reason 取值（A7，计划 §3.10）。
const COMPONENT_CHANGE_REASONS = new Set(['files', 'state', 'instances', 'deleted', 'created'])

export class BoardEventsLimitError extends Error {
  constructor() {
    super('看板事件流连接数已达上限。')
    this.name = 'BoardEventsLimitError'
    this.code = 'board_events_limit'
    this.statusCode = 429
  }
}

/** @param {unknown} data */
function estimateBytes(data) {
  try {
    return JSON.stringify(data)?.length ?? 0
  } catch {
    return 0
  }
}

export class BoardEventHub {
  /** @param {BoardEventHubOptions} options */
  constructor({
    listItems,
    snapshotLimit = 500,
    minIntervalMs = BOARD_CARD_MIN_INTERVAL_MS,
    coalesceMs = DEFAULT_COALESCE_MS,
    minFlushGapMs = DEFAULT_MIN_FLUSH_GAP_MS,
    heartbeatMs = BOARD_HEARTBEAT_MS,
    maxBufferedEvents = DEFAULT_MAX_BUFFERED_EVENTS,
    maxBufferedBytes = DEFAULT_MAX_BUFFERED_BYTES,
    maxDirty = DEFAULT_MAX_DIRTY,
    maxConnections = BOARD_EVENTS_MAX_CONNECTIONS,
    maxQueuedPerConnection = DEFAULT_MAX_QUEUED_PER_CONNECTION,
    now = Date.now,
    onError,
  }) {
    this.listItems = listItems
    this.snapshotLimit = snapshotLimit
    this.minIntervalMs = minIntervalMs
    this.coalesceMs = coalesceMs
    this.minFlushGapMs = minFlushGapMs
    this.lastFlushAt = Number.NEGATIVE_INFINITY
    this.heartbeatMs = heartbeatMs
    this.maxBufferedEvents = maxBufferedEvents
    this.maxBufferedBytes = maxBufferedBytes
    this.maxDirty = maxDirty
    this.maxConnections = maxConnections
    this.maxQueuedPerConnection = maxQueuedPerConnection
    this.now = now
    this.onError = onError || null
    this.revision = Math.max(1, Math.floor(now()))
    // 早于（含）这个修订号的增量已经无法补发：缓冲被挤出，或空闲期脏标记溢出。
    this.droppedUpTo = this.revision
    /** @type {BufferedEvent[]} */
    this.buffer = []
    this.bufferedBytes = 0
    /** @type {Set<string>} */
    this.dirty = new Set()
    /** @type {Map<string, number>} */
    this.lastSentAt = new Map()
    /** @type {Map<string, boolean>} */
    this.eligibility = new Map()
    /** @type {Set<BoardConnection>} */
    this.connections = new Set()
    /** @type {ReturnType<typeof setTimeout> | null} */
    this.flushTimer = null
    /** @type {ReturnType<typeof setInterval> | null} */
    this.heartbeatTimer = null
    /** @type {Promise<void>} */
    this.queue = Promise.resolve()
    this.disposed = false
  }

  /** 会话卡片可能变化：只记脏标记，有连接时再安排 flush。 @param {string} sessionId */
  touch(sessionId) {
    if (this.disposed || typeof sessionId !== 'string' || !sessionId) return
    if (!this.dirty.has(sessionId) && this.dirty.size >= this.maxDirty) {
      // 记不下哪些会话变了：清空并把修订号推进一格作为缺口，带旧修订号重连的客户端会收到 resync_required。
      this.dirty.clear()
      this.revision += 1
      this.droppedUpTo = this.revision
      this.eligibility.clear()
    }
    this.dirty.add(sessionId)
    if (this.connections.size) this.scheduleFlush(this.coalesceMs)
  }

  /**
   * 组件变化通知。revision 仍是看板修订号；A7 只做加法：rev 是组件内容哈希（只有 ready 的组件有），
   * reason 说明变化来源（files / state / instances / deleted / created），旧客户端忽略这两个字段。
   * @param {string} componentId
   * @param {{ rev?: string | null, reason?: string }} [detail]
   */
  componentChanged(componentId, { rev, reason } = {}) {
    if (this.disposed || typeof componentId !== 'string' || !componentId) return
    /** @type {{ rev?: string, reason?: string }} */
    const extra = {}
    if (typeof rev === 'string' && /^[a-f0-9]{24}$/.test(rev)) extra.rev = rev
    if (typeof reason === 'string' && COMPONENT_CHANGE_REASONS.has(reason)) extra.reason = reason
    this.enqueue(async () => {
      this.publish('component_changed', '', (revision) => ({
        id: componentId,
        revision,
        ...extra,
      }))
    })
  }

  /** @param {number} delay */
  scheduleFlush(delay) {
    if (this.flushTimer || this.disposed) return
    delay = Math.max(delay, this.lastFlushAt + this.minFlushGapMs - this.now())
    this.flushTimer = setTimeout(
      () => {
        this.flushTimer = null
        this.enqueue(() => this.flush(false))
      },
      Math.max(0, delay),
    )
    this.flushTimer.unref?.()
  }

  /** 串行执行：flush 与快照不会交错，连接看到的修订号顺序与生成顺序一致。 @param {() => Promise<void>} task */
  enqueue(task) {
    const next = this.queue.then(task).catch((error) => {
      this.onError?.(error)
    })
    this.queue = next
    return next
  }

  /**
   * @param {string} event
   * @param {string} id
   * @param {(revision: number) => Record<string, unknown>} build
   */
  publish(event, id, build) {
    this.revision += 1
    const data = build(this.revision)
    const entry = { revision: this.revision, event, data, bytes: estimateBytes(data), id }
    this.buffer.push(entry)
    this.bufferedBytes += entry.bytes
    while (
      this.buffer.length > this.maxBufferedEvents ||
      (this.bufferedBytes > this.maxBufferedBytes && this.buffer.length > 1)
    ) {
      const evicted = this.buffer.shift()
      if (!evicted) break
      this.bufferedBytes -= evicted.bytes
      // 缺口只能前移：空闲期溢出留下的缺口可能比被挤出的帧更新。
      this.droppedUpTo = Math.max(this.droppedUpTo, evicted.revision)
    }
    for (const connection of this.connections) this.deliver(connection, entry)
  }

  /** @param {BoardConnection} connection @param {BufferedEvent} entry */
  deliver(connection, entry) {
    if (connection.closed) return
    if (!connection.ready) {
      // 快照计算期间产生的增量先排队；队列爆掉说明连接卡住，直接断开让客户端重连。
      if (connection.queued.length >= this.maxQueuedPerConnection) this.closeConnection(connection)
      else connection.queued.push(entry)
      return
    }
    try {
      if (connection.send(entry.event, entry.data) === false) this.closeConnection(connection)
    } catch {
      this.closeConnection(connection)
    }
  }

  /** @param {boolean} force 连接建立时忽略节流，把积压的脏会话一次算完 */
  async flush(force) {
    if (this.disposed || !this.dirty.size) return
    const now = this.now()
    this.lastFlushAt = now
    const due = []
    let wait = Number.POSITIVE_INFINITY
    for (const id of this.dirty) {
      const elapsed = now - (this.lastSentAt.get(id) ?? Number.NEGATIVE_INFINITY)
      if (force || elapsed >= this.minIntervalMs) due.push(id)
      else wait = Math.min(wait, this.minIntervalMs - elapsed)
    }
    for (const id of due) this.dirty.delete(id)
    if (due.length) {
      let items
      try {
        items = await this.listItems(due)
      } catch (error) {
        // 读不到会话列表时无法生成增量：推进缺口并让在线客户端重新同步，而不是悄悄丢掉变化。
        this.onError?.(error)
        this.publish('resync_required', '', (revision) => ({ revision, reason: 'flush_failed' }))
        this.droppedUpTo = this.revision
        items = null
      }
      if (items) {
        const sentAt = this.now()
        for (const id of due) {
          this.lastSentAt.set(id, sentAt)
          this.emitCard(id, items.get(id) ?? null)
        }
      }
    }
    // 节流记录只需要覆盖最近一个间隔窗口，过期的直接清掉，避免随会话数增长。
    for (const [id, sentAt] of this.lastSentAt) {
      if (now - sentAt >= this.minIntervalMs) this.lastSentAt.delete(id)
    }
    if (this.dirty.size && this.connections.size)
      this.scheduleFlush(Number.isFinite(wait) ? wait : this.coalesceMs)
  }

  /** @param {string} id @param {BoardHubItem} item */
  emitCard(id, item) {
    if (item) {
      this.remember(id, true)
      this.publish('session_card', id, (revision) => ({
        revision,
        id,
        card: item.card,
        session: item.session,
      }))
      return
    }
    // 不上看板（归档、删除、出了时间窗）：只有之前可能在看板上的才发 removed；未知状态按可能在处理。
    if (this.eligibility.get(id) === false) return
    this.remember(id, false)
    this.publish('session_card', id, (revision) => ({ revision, id, removed: true }))
  }

  /** @param {string} id @param {boolean} eligible */
  remember(id, eligible) {
    this.eligibility.delete(id)
    this.eligibility.set(id, eligible)
    while (this.eligibility.size > MAX_ELIGIBILITY_ENTRIES) {
      const oldest = this.eligibility.keys().next().value
      if (oldest === undefined) break
      this.eligibility.delete(oldest)
    }
  }

  /**
   * 建立一条连接。after 为客户端上次见到的修订号（可选）：
   * - 没有 after：立即发 snapshot；
   * - after 落在可补发范围内：按会话合并后补发缺失的增量；
   * - 有缺口（缓冲已挤出、Runtime 重启、修订号来自未来）：先发 resync_required，再发 snapshot。
   * @param {{ after?: number | null, send: BoardConnection['send'], close: BoardConnection['close'] }} options
   * @returns {{ ready: Promise<void>, detach: () => void }}
   */
  connect({ after = null, send, close }) {
    if (this.disposed || this.connections.size >= this.maxConnections)
      throw new BoardEventsLimitError()
    /** @type {BoardConnection} */
    const connection = { send, close, ready: false, queued: [], closed: false }
    this.connections.add(connection)
    this.startHeartbeat()
    const ready = this.enqueue(async () => {
      try {
        await this.attach(connection, after)
      } catch (error) {
        // 快照读不出来时不能让连接一直停在“未就绪”：断开，客户端稍后重连再试。
        this.onError?.(error)
        this.closeConnection(connection)
      }
    })
    return { ready, detach: () => this.closeConnection(connection, false) }
  }

  /** @param {BoardConnection} connection @param {number | null} after */
  async attach(connection, after) {
    if (connection.closed) return
    await this.flush(true)
    if (connection.closed) return
    const replayable =
      after !== null && after >= this.droppedUpTo && after <= this.revision ? after : null
    let baseline = this.revision
    if (replayable !== null) {
      this.replay(connection, replayable)
    } else {
      if (after !== null)
        this.sendTo(connection, 'resync_required', {
          revision: this.revision,
          reason: after > this.revision ? 'unknown_revision' : 'gap',
        })
      baseline = await this.sendSnapshot(connection)
    }
    if (connection.closed) return
    connection.ready = true
    // 排队期间的增量里，修订号不超过快照 / 补发基线的已经包含在里面，再发一遍可能用旧状态覆盖新快照。
    const queued = connection.queued.splice(0)
    for (const entry of queued) if (entry.revision > baseline) this.deliver(connection, entry)
  }

  /** @param {BoardConnection} connection @param {number} after */
  replay(connection, after) {
    // 同一会话只补最后一帧：中间状态对客户端没有意义，补发量与变化的会话数成正比。
    /** @type {Map<string, BufferedEvent>} */
    const latest = new Map()
    const others = []
    for (const entry of this.buffer) {
      if (entry.revision <= after) continue
      if (entry.event === 'session_card' && entry.id) {
        latest.delete(entry.id)
        latest.set(entry.id, entry)
      } else others.push(entry)
    }
    const entries = [...latest.values(), ...others].sort(
      (left, right) => left.revision - right.revision,
    )
    for (const entry of entries) this.sendTo(connection, entry.event, entry.data)
    // 补发结束也给出当前修订号，客户端据此更新 after，即使期间没有任何变化。
    this.sendTo(connection, 'heartbeat', { revision: this.revision, at: this.isoNow() })
  }

  /** @param {BoardConnection} connection @returns {Promise<number>} 快照对应的修订号 */
  async sendSnapshot(connection) {
    const revision = this.revision
    const items = await this.listItems(null)
    if (connection.closed) return revision
    const all = [...items.values()].filter((item) => item !== null)
    for (const [id, item] of items) this.remember(id, item !== null)
    const limited = all.slice(0, this.snapshotLimit)
    this.sendTo(connection, 'snapshot', {
      revision,
      items: limited.map((item) => ({ ...item.session, card: item.card })),
      ...(all.length > limited.length ? { truncated: true } : {}),
    })
    return revision
  }

  /** @param {BoardConnection} connection @param {string} event @param {Record<string, unknown>} data */
  sendTo(connection, event, data) {
    if (connection.closed) return
    try {
      if (connection.send(event, data) === false) this.closeConnection(connection)
    } catch {
      this.closeConnection(connection)
    }
  }

  startHeartbeat() {
    if (this.heartbeatTimer || this.disposed) return
    this.heartbeatTimer = setInterval(() => {
      const data = { revision: this.revision, at: this.isoNow() }
      for (const connection of this.connections) {
        if (connection.ready) this.sendTo(connection, 'heartbeat', data)
      }
    }, this.heartbeatMs)
    this.heartbeatTimer.unref?.()
  }

  isoNow() {
    return new Date(this.now()).toISOString()
  }

  /** @param {BoardConnection} connection @param {boolean} [notify] */
  closeConnection(connection, notify = true) {
    if (connection.closed) return
    connection.closed = true
    connection.queued.length = 0
    this.connections.delete(connection)
    if (notify) {
      try {
        connection.close()
      } catch {
        // 关闭失败只影响这一条连接。
      }
    }
    if (!this.connections.size) this.stopTimers()
  }

  stopTimers() {
    if (this.heartbeatTimer) clearInterval(this.heartbeatTimer)
    this.heartbeatTimer = null
    if (this.flushTimer) clearTimeout(this.flushTimer)
    this.flushTimer = null
  }

  /** 供诊断与测试：当前连接数与缓冲占用。 */
  stats() {
    return {
      connections: this.connections.size,
      buffered: this.buffer.length,
      bufferedBytes: this.bufferedBytes,
      dirty: this.dirty.size,
      revision: this.revision,
      timers: Boolean(this.heartbeatTimer) || Boolean(this.flushTimer),
    }
  }

  dispose() {
    this.disposed = true
    for (const connection of [...this.connections]) this.closeConnection(connection)
    this.stopTimers()
    this.buffer = []
    this.bufferedBytes = 0
    this.dirty.clear()
    this.lastSentAt.clear()
    this.eligibility.clear()
  }
}
