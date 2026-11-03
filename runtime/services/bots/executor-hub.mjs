// 执行节点的连接中枢（B6）：节点主动连回大脑（长轮询，不需要节点开放端口，也能穿过 NAT），
// 领走要执行的请求，再把结果和命令输出交回来。大脑这边只排队、配对请求与结果，不保存节点的任何文件。
// - 在线：节点正挂着一次轮询，或最近 ONLINE_WINDOW_MS 内轮询过。
// - 节点离线时新请求立即失败（executor_offline）；已发出的请求在超时后失败，调用方据此把任务记为中断。
// - 中止（AbortSignal）时给节点发一条 cancel，节点停掉对应命令。
import { randomBytes } from 'node:crypto'
import { ExecutorError } from './executor.mjs'

export const POLL_WAIT_MS = 25_000
const ONLINE_WINDOW_MS = 40_000
const DEFAULT_CALL_TIMEOUT_MS = 120_000
const MAX_QUEUED_PER_ENVIRONMENT = 64
const MAX_REQUESTS_PER_POLL = 16

export class ExecutorHub {
  constructor({ now = () => Date.now(), onOnline = () => {} } = {}) {
    this.now = now
    this.onOnline = onOnline
    this.queues = new Map()
    this.waiters = new Map()
    this.lastPollAt = new Map()
    this.pending = new Map()
  }

  online(environmentId) {
    if (this.waiters.has(environmentId)) return true
    const last = this.lastPollAt.get(environmentId)
    return typeof last === 'number' && this.now() - last < ONLINE_WINDOW_MS
  }

  #take(environmentId) {
    const queue = this.queues.get(environmentId) || []
    const batch = queue.splice(0, MAX_REQUESTS_PER_POLL)
    if (!queue.length) this.queues.delete(environmentId)
    return batch
  }

  #wake(environmentId) {
    const waiter = this.waiters.get(environmentId)
    if (!waiter) return
    this.waiters.delete(environmentId)
    clearTimeout(waiter.timer)
    waiter.resolve(this.#take(environmentId))
  }

  // 节点的一次轮询：有请求立即返回，否则最多等 waitMs。同一节点只挂一个轮询，新的顶掉旧的。
  poll(environmentId, { signal, waitMs = POLL_WAIT_MS } = {}) {
    const wasOnline = this.online(environmentId)
    this.lastPollAt.set(environmentId, this.now())
    const previous = this.waiters.get(environmentId)
    if (previous) {
      this.waiters.delete(environmentId)
      clearTimeout(previous.timer)
      previous.resolve([])
    }
    if (!wasOnline) queueMicrotask(() => this.onOnline(environmentId))
    const ready = this.#take(environmentId)
    if (ready.length) return Promise.resolve(ready)
    return new Promise((resolve) => {
      const finish = () => {
        if (this.waiters.get(environmentId)?.resolve !== resolve) return
        this.waiters.delete(environmentId)
        clearTimeout(waiter.timer)
        // 轮询结束也算一次活动：下一次轮询通常紧接着就来。
        this.lastPollAt.set(environmentId, this.now())
        resolve([])
      }
      const waiter = { resolve, timer: setTimeout(finish, waitMs) }
      this.waiters.set(environmentId, waiter)
      signal?.addEventListener('abort', finish, { once: true })
    })
  }

  #send(environmentId, request) {
    const queue = this.queues.get(environmentId) || []
    if (queue.length >= MAX_QUEUED_PER_ENVIRONMENT)
      throw new ExecutorError('executor_busy', '这台电脑上排队的操作太多了，稍后再试。')
    queue.push(request)
    this.queues.set(environmentId, queue)
    this.#wake(environmentId)
  }

  // 发一个请求给节点并等结果。onChunk 收命令的实时输出（Buffer）。
  call(environmentId, op, args, { signal, timeoutMs = DEFAULT_CALL_TIMEOUT_MS, onChunk } = {}) {
    if (signal?.aborted) return Promise.reject(new ExecutorError('aborted', '操作已取消。'))
    if (!this.online(environmentId))
      return Promise.reject(
        new ExecutorError('executor_offline', '这台电脑现在不在线，等它连上后再继续。'),
      )
    const id = `req_${randomBytes(9).toString('base64url')}`
    return new Promise((resolve, reject) => {
      const settle = (error, value) => {
        const entry = this.pending.get(id)
        if (!entry) return
        this.pending.delete(id)
        clearTimeout(entry.timer)
        signal?.removeEventListener('abort', abort)
        if (error) reject(error)
        else resolve(value)
      }
      const abort = () => {
        try {
          this.#send(environmentId, { id: `${id}:cancel`, op: 'cancel', args: { id } })
        } catch {}
        settle(new ExecutorError('aborted', '操作已取消。'))
      }
      const timer = setTimeout(
        () => settle(new ExecutorError('executor_timeout', '这台电脑太久没有回应，可能已经断线。')),
        timeoutMs,
      )
      this.pending.set(id, { environmentId, onChunk, settle, timer })
      signal?.addEventListener('abort', abort, { once: true })
      try {
        this.#send(environmentId, { id, op, args })
      } catch (error) {
        settle(error)
      }
    })
  }

  // 节点交回的结果与输出：只接受发给这个节点的请求，其余忽略。
  deliver(environmentId, results) {
    let accepted = 0
    for (const raw of Array.isArray(results) ? results : []) {
      const result = raw && typeof raw === 'object' ? raw : {}
      const entry = this.pending.get(String(result.id ?? ''))
      if (!entry || entry.environmentId !== environmentId) continue
      accepted += 1
      if (typeof result.chunk === 'string') {
        try {
          entry.onChunk?.(Buffer.from(result.chunk, 'base64'))
        } catch {}
        continue
      }
      if (result.ok === true) entry.settle(null, result.value)
      else {
        const error = result.error && typeof result.error === 'object' ? result.error : {}
        entry.settle(
          new ExecutorError(
            String(error.code || 'executor_failed').slice(0, 60),
            String(error.message || '执行节点上的操作失败了。').slice(0, 500),
          ),
        )
      }
    }
    return accepted
  }

  // 节点被移除：挂着的轮询与等结果的请求全部结束。
  disconnect(environmentId) {
    const waiter = this.waiters.get(environmentId)
    if (waiter) {
      this.waiters.delete(environmentId)
      clearTimeout(waiter.timer)
      waiter.resolve([])
    }
    this.queues.delete(environmentId)
    this.lastPollAt.delete(environmentId)
    for (const entry of [...this.pending.values()])
      if (entry.environmentId === environmentId)
        entry.settle(new ExecutorError('executor_offline', '这台电脑已经移除。'))
  }

  dispose() {
    for (const id of [...this.waiters.keys()]) this.disconnect(id)
    for (const entry of [...this.pending.values()])
      entry.settle(new ExecutorError('executor_offline', '应用正在关闭。'))
  }
}
