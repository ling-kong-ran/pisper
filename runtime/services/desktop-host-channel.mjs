// Runtime 和桌面前端之间的指令通道：Runtime 碰不到原生能力，指令经 SSE 发给桌面前端，
// 它调用桥接执行，结果再 POST 回来。右侧浏览器和 Computer Use 各用一条。
import { randomUUID } from 'node:crypto'

const REQUEST_TIMEOUT_MS = 60_000

export class DesktopHostChannel {
  constructor({
    timeoutMs = REQUEST_TIMEOUT_MS,
    setTimer = setTimeout,
    clearTimer = clearTimeout,
  } = {}) {
    this.timeoutMs = timeoutMs
    this.setTimer = setTimer
    this.clearTimer = clearTimer
    this.host = null
    this.pending = new Map()
  }

  get available() {
    return Boolean(this.host)
  }

  // 只有最新的连接算数：前端刷新后旧连接可能还没断开。
  attach(send) {
    this.failPending('The desktop app reconnected before finishing this action.')
    const host = { send }
    this.host = host
    return () => {
      if (this.host !== host) return
      this.host = null
      this.failPending('The desktop app disconnected before finishing this action.')
    }
  }

  failPending(message) {
    for (const entry of [...this.pending.values()]) entry.finish(new Error(message))
  }

  request(sessionId, command, signal) {
    return new Promise((resolve, reject) => {
      const requestId = randomUUID()
      const abort = () => {
        this.host?.send('cancel', { sessionId })
        entry.finish(signal.reason instanceof Error ? signal.reason : new Error('Aborted'))
      }
      const timer = this.setTimer(() => {
        entry.finish(new Error(`The action "${command.action}" timed out.`))
      }, this.timeoutMs)
      timer?.unref?.()
      const entry = {
        finish: (error, value) => {
          this.pending.delete(requestId)
          this.clearTimer(timer)
          signal?.removeEventListener?.('abort', abort)
          if (error) reject(error)
          else resolve(value)
        },
      }
      this.pending.set(requestId, entry)
      signal?.addEventListener?.('abort', abort, { once: true })
      this.host.send('command', { requestId, sessionId, ...command })
    })
  }

  // 晚到或未知的结果直接丢弃：那个请求已经超时或被取消了。
  settle(requestId, payload) {
    const entry = this.pending.get(String(requestId))
    if (!entry) return false
    const record = payload && typeof payload === 'object' ? payload : {}
    if (record.ok === true) entry.finish(null, record.result ?? {})
    else {
      const message = typeof record.error === 'string' ? record.error.slice(0, 500) : ''
      entry.finish(new Error(message || 'The action failed.'))
    }
    return true
  }

  dispose() {
    this.failPending('The desktop app connection is shutting down.')
    this.host = null
  }
}
