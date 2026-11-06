// 桌面端右侧浏览器的指令通道：Runtime 碰不到原生 WebView，指令经 SSE 发给桌面前端，
// 由它调用桥接执行，结果再 POST 回来。前端连着时 browser_automation 走这里，用户看得见
// Agent 在操作；没连（Web、Linux、远程窗口）时由 BrowserAutomationService 退回无头浏览器。
import { randomUUID } from 'node:crypto'
import { writeFile } from 'node:fs/promises'
import { basename } from 'node:path'

const REQUEST_TIMEOUT_MS = 60_000
const MAX_PAGE_TEXT_CHARS = 20_000
const MAX_ELEMENTS = 100
const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47])

function text(value, maximum) {
  return typeof value === 'string' ? value.slice(0, maximum) : ''
}

// 结果来自前端，内容来自网页：只留已知字段并限制长度，不把任意数据交给模型。
function pageResult(value) {
  return { url: text(value.url, 8_192), title: text(value.title, 500) }
}

function elementResult(value) {
  const record = value && typeof value === 'object' ? value : {}
  return {
    selector: text(record.selector, 500),
    tag: text(record.tag, 40),
    role: text(record.role, 40),
    text: text(record.text, 240),
    href: text(record.href, 2_000),
    type: text(record.type, 40),
    disabled: record.disabled === true,
  }
}

export class DesktopBrowserHostService {
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
    this.failPending('The desktop browser reconnected before finishing this action.')
    const host = { send }
    this.host = host
    return () => {
      if (this.host !== host) return
      this.host = null
      this.failPending('The desktop browser disconnected before finishing this action.')
    }
  }

  failPending(message) {
    for (const entry of [...this.pending.values()]) entry.finish(new Error(message))
  }

  async execute(sessionId, input, { signal } = {}) {
    if (!this.host) throw new Error('The desktop browser is not connected.')
    const value = await this.request(sessionId, input, signal)
    return this.result(input, value)
  }

  request(sessionId, input, signal) {
    return new Promise((resolve, reject) => {
      const requestId = randomUUID()
      const abort = () => {
        this.host?.send('cancel', { sessionId })
        entry.finish(signal.reason instanceof Error ? signal.reason : new Error('Aborted'))
      }
      const timer = this.setTimer(() => {
        entry.finish(new Error(`The browser action "${input.action}" timed out.`))
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
      this.host.send('command', {
        requestId,
        sessionId,
        action: input.action,
        url: input.url,
        selector: input.selector,
        text: input.text,
        submit: input.submit === true,
        waitMs: input.waitMs,
      })
    })
  }

  // 晚到或未知的结果直接丢弃：那个请求已经超时或被取消了。
  settle(requestId, payload) {
    const entry = this.pending.get(String(requestId))
    if (!entry) return false
    const record = payload && typeof payload === 'object' ? payload : {}
    if (record.ok === true) entry.finish(null, record.result ?? {})
    else entry.finish(new Error(text(record.error, 500) || 'The browser action failed.'))
    return true
  }

  async result(input, value) {
    const record = value && typeof value === 'object' ? value : {}
    const { action } = input
    if (action === 'close') return { action, closed: true }
    if (action === 'inspect') {
      return {
        action,
        ...pageResult(record),
        text: text(record.text, MAX_PAGE_TEXT_CHARS),
        elements: (Array.isArray(record.elements) ? record.elements : [])
          .slice(0, MAX_ELEMENTS)
          .map(elementResult),
      }
    }
    if (action === 'screenshot') {
      const image = Buffer.from(text(record.imageBase64, 64_000_000), 'base64')
      if (!image.subarray(0, 4).equals(PNG_SIGNATURE))
        throw new Error('The desktop browser did not return a PNG screenshot.')
      await writeFile(input.outputPath, image)
      return {
        action,
        path: input.outputPath,
        name: basename(input.outputPath),
        mimeType: 'image/png',
        ...pageResult(record),
        // 原生截图只有可视区域。
        fullPage: false,
      }
    }
    if (action === 'click') return { action, selector: input.selector, ...pageResult(record) }
    if (action === 'type')
      return {
        action,
        selector: input.selector,
        submitted: input.submit === true,
        ...pageResult(record),
      }
    if (action === 'wait')
      return { action, waitMs: Number(record.waitMs) || 0, ...pageResult(record) }
    return { action, ...pageResult(record) }
  }

  // 空闲超时或工具 close：把这一页还给用户，页签留着。
  closeSession(sessionId) {
    this.host?.send('release', { sessionId })
    return true
  }

  dispose() {
    this.failPending('The desktop browser host is shutting down.')
    this.host = null
  }
}
