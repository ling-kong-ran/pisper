// 移动设备操作协调器：把 Agent 工具调用绑定到当前手机 SSE 客户端，
// 等待 App 完成系统授权与原生操作后，再把受限结果返回给该工具。
import { randomUUID } from 'node:crypto'

const DEFAULT_TIMEOUT_MS = 120_000
const MAX_RESULT_BYTES = 12 * 1024 * 1024

function operationId() {
  return `mop_${randomUUID().replaceAll('-', '')}`
}

function resultSize(value) {
  try {
    return Buffer.byteLength(JSON.stringify(value), 'utf8')
  } catch {
    return MAX_RESULT_BYTES + 1
  }
}

export class MobileOperationService {
  constructor({ timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
    this.timeoutMs = timeoutMs
    this.channels = new Map()
    this.pending = new Map()
  }

  attach(sessionId, emit) {
    if (!sessionId || typeof emit !== 'function') return () => {}
    const channel = { emit, token: Symbol(sessionId) }
    this.channels.set(sessionId, channel)
    return () => {
      if (this.channels.get(sessionId)?.token === channel.token) this.channels.delete(sessionId)
    }
  }

  async execute(sessionId, operation, parameters = {}, { signal } = {}) {
    const channel = this.channels.get(sessionId)
    if (!channel) throw new Error('当前会话没有已连接的 Pisper 移动 App。')
    if (signal?.aborted) throw new Error('移动设备操作已取消。')

    const id = operationId()
    const expiresAt = new Date(Date.now() + this.timeoutMs).toISOString()
    const emitCancellation = (reason) => {
      try {
        channel.emit('mobile_operation_cancel', { id, sessionId, reason })
      } catch {
        // SSE 连接已经关闭时，Runtime 的挂起状态仍由本地超时/中止负责清理。
      }
    }
    const pending = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id)
        emitCancellation('timeout')
        reject(new Error('等待手机完成设备操作超时。'))
      }, this.timeoutMs)
      timer.unref?.()
      const abort = () => {
        clearTimeout(timer)
        this.pending.delete(id)
        emitCancellation('aborted')
        reject(new Error('移动设备操作已取消。'))
      }
      signal?.addEventListener('abort', abort, { once: true })
      this.pending.set(id, {
        sessionId,
        resolve,
        reject,
        timer,
        cleanup: () => signal?.removeEventListener('abort', abort),
        cancel: emitCancellation,
      })
    })

    channel.emit('mobile_operation_request', {
      id,
      sessionId,
      operation,
      parameters,
      expiresAt,
    })
    return pending
  }

  resolve(sessionId, id, input = {}) {
    const pending = this.pending.get(id)
    if (!pending || pending.sessionId !== sessionId) return false
    this.pending.delete(id)
    clearTimeout(pending.timer)
    pending.cleanup()
    if (input.ok === false) {
      pending.reject(new Error(String(input.error || '手机拒绝了设备操作。').slice(0, 2_000)))
      return true
    }
    if (resultSize(input.result) > MAX_RESULT_BYTES) {
      pending.reject(new Error('手机设备操作结果超过 12 MB 限制。'))
      return true
    }
    pending.resolve(input.result ?? null)
    return true
  }

  dispose() {
    this.channels.clear()
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer)
      pending.cleanup()
      pending.cancel('runtime_disposed')
      pending.reject(new Error('Runtime 已关闭，移动设备操作已取消。'))
    }
    this.pending.clear()
  }
}
