// Run 注册表：把一次流式执行（POST /api/chat）抽象为带单调游标的 run，
// 事件帧进入环形缓冲，支持断线后按游标重挂（GET /api/runs/:id/events?after=）。
// run 的生命周期与连接解耦：客户端断开不终止 run，缓冲继续累积，
// 终态后保留一段时间供重放，随后清理。
import { randomBytes } from 'node:crypto'

const DEFAULT_MAX_EVENTS = 512
const DEFAULT_MAX_BYTES = 1024 * 1024
const DEFAULT_RETENTION_MS = 10 * 60 * 1000
// 兜底寿命：run 若因异常始终没有关闭，最多保留一天，防止泄漏。
const DEFAULT_MAX_AGE_MS = 24 * 60 * 60 * 1000

export class RunNotResumableError extends Error {
  constructor(message = '该运行已结束且缓存已清理，无法续传。') {
    super(message)
    this.name = 'RunNotResumableError'
    this.code = 'run_not_resumable'
  }
}

export class RunRegistry {
  constructor({
    maxEvents = DEFAULT_MAX_EVENTS,
    maxBytes = DEFAULT_MAX_BYTES,
    retentionMs = DEFAULT_RETENTION_MS,
    maxAgeMs = DEFAULT_MAX_AGE_MS,
  } = {}) {
    this.maxEvents = maxEvents
    this.maxBytes = maxBytes
    this.retentionMs = retentionMs
    this.maxAgeMs = maxAgeMs
    this.runs = new Map()
  }

  begin(meta = {}) {
    const run = {
      id: `run_${randomBytes(9).toString('base64url')}`,
      meta,
      cursor: 0,
      events: [],
      bytes: 0,
      // 已被挤出缓冲的最大游标：重挂时 after 小于它说明存在缺口。
      droppedUpTo: 0,
      closed: false,
      sinks: new Set(),
      closeWaiters: new Set(),
      createdAt: Date.now(),
      cleanupTimer: null,
    }
    // 兜底寿命计时：无论是否关闭，到期强制清理。
    run.cleanupTimer = setTimeout(() => this.drop(run.id), this.maxAgeMs)
    run.cleanupTimer.unref?.()
    this.runs.set(run.id, run)
    return run
  }

  get(runId) {
    return this.runs.get(runId) || null
  }

  // 记录一帧：分配游标、入缓冲、广播给重挂订阅者。返回游标；run 已关闭返回 null。
  record(run, event, data) {
    if (!run || run.closed) return null
    const cursor = ++run.cursor
    // 字节数用于缓冲上限控制，只需量级准确，不必精确。
    const bytes = 16 + event.length + Buffer.byteLength(safeSerialize(data))
    run.events.push({ cursor, event, data, bytes })
    run.bytes += bytes
    while (run.events.length > this.maxEvents || run.bytes > this.maxBytes) {
      const evicted = run.events.shift()
      if (!evicted) break
      run.bytes -= evicted.bytes
      run.droppedUpTo = evicted.cursor
    }
    for (const sink of [...run.sinks]) {
      try {
        sink.onEvent(cursor, event, data)
      } catch {
        run.sinks.delete(sink)
      }
    }
    return cursor
  }

  // 关闭 run：通知所有重挂订阅者结束，并安排终态缓存的到期清理。
  close(run) {
    if (!run || run.closed) return
    run.closed = true
    for (const sink of [...run.sinks]) {
      try {
        sink.onEnd?.()
      } catch {
        // 订阅者异常不影响关闭。
      }
    }
    run.sinks.clear()
    for (const resolve of [...run.closeWaiters]) resolve()
    run.closeWaiters.clear()
    clearTimeout(run.cleanupTimer)
    run.cleanupTimer = setTimeout(() => this.drop(run.id), this.retentionMs)
    run.cleanupTimer.unref?.()
  }

  // 重挂第一步：取出 after 之后的缓存帧与缺口标记。
  // gap 为真表示缓冲已溢出（需要的旧事件已被挤出），调用方应在补发前先插 resync_required。
  prepareAttach(runId, after) {
    const run = this.runs.get(runId)
    if (!run) throw new RunNotResumableError()
    const gap = run.droppedUpTo > after
    const replay = run.events.filter((entry) => entry.cursor > after)
    return { run, gap, replay }
  }

  // 重挂第二步：补发完成后订阅实时帧。run 已关闭返回 false（回放即全部）。
  // 与 prepareAttach 之间是同步代码，不会错过中间事件。
  subscribe(run, sink) {
    if (run.closed) return false
    run.sinks.add(sink)
    return true
  }

  detach(run, sink) {
    run?.sinks.delete(sink)
  }

  // 等待 run 关闭（重挂路由用它挂起响应直到流结束）。
  waitForClose(run) {
    if (run.closed) return Promise.resolve()
    return new Promise((resolve) => run.closeWaiters.add(resolve))
  }

  drop(runId) {
    const run = this.runs.get(runId)
    if (!run) return
    clearTimeout(run.cleanupTimer)
    this.runs.delete(runId)
  }
}

function safeSerialize(data) {
  try {
    return JSON.stringify(data) || ''
  } catch {
    return ''
  }
}
