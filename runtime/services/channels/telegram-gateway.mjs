// Telegram 网关：通过 Bot API 长轮询收发消息，凭据仅保存在渠道服务的私有状态中。
import { readFile } from 'node:fs/promises'

const DEFAULT_BASE_URL = 'https://api.telegram.org'
const DEFAULT_FILE_BASE_URL = 'https://api.telegram.org/file'

function abortableDelay(ms, signal) {
  if (signal.aborted) return Promise.resolve()
  return new Promise((resolve) => {
    const timer = setTimeout(done, ms)
    timer.unref?.()
    function done() {
      clearTimeout(timer)
      signal.removeEventListener('abort', done)
      resolve()
    }
    signal.addEventListener('abort', done, { once: true })
  })
}

function safeError(error) {
  const message = error instanceof Error ? error.message : String(error)
  return message.replace(/bot\d+:[A-Za-z0-9_-]+/gi, 'bot***:***').slice(0, 1000)
}

function validToken(value) {
  const token = String(value || '').trim()
  if (!token || !/^\d+:[A-Za-z0-9_-]+$/.test(token))
    throw new Error('Telegram Bot Token 格式无效。')
  return token
}

export class TelegramGateway {
  constructor({
    fetchImpl = globalThis.fetch,
    onMessage = () => {},
    onStatusChange = () => {},
  } = {}) {
    if (typeof fetchImpl !== 'function') throw new Error('Telegram 网关需要可用的 fetch 实现。')
    this.fetchImpl = fetchImpl
    this.onMessage = onMessage
    this.onStatusChange = onStatusChange
    this.connection = null
    this.controller = null
    this.monitorPromise = null
    this.offset = 0
    this.status = { state: 'idle', lastError: '', connectedAt: null, lastEventAt: null, bot: null }
  }

  setStatus(patch) {
    this.status = { ...this.status, ...patch }
    this.onStatusChange(this.getStatus())
  }

  getStatus() {
    return { ...this.status }
  }

  apiUrl(method, token = this.connection?.token, baseUrl = this.connection?.baseUrl) {
    return `${String(baseUrl || DEFAULT_BASE_URL).replace(/\/$/, '')}/bot${validToken(token)}/${method}`
  }

  async request(method, body, { signal, multipart = false } = {}) {
    let response
    try {
      response = await this.fetchImpl(this.apiUrl(method), {
        method: 'POST',
        headers: multipart ? undefined : { 'content-type': 'application/json' },
        body: multipart ? body : JSON.stringify(body || {}),
        signal,
      })
    } catch (error) {
      throw new Error(`Telegram ${method} 请求失败：${safeError(error)}`)
    }
    let payload
    try {
      payload = await response.json()
    } catch {
      throw new Error(`Telegram ${method} 返回了无效响应（HTTP ${response.status}）。`)
    }
    if (!response.ok || payload?.ok !== true) {
      const description = String(payload?.description || `HTTP ${response.status}`)
      throw new Error(`Telegram ${method} 失败：${description.slice(0, 500)}`)
    }
    return payload.result
  }

  async getMe(signal) {
    return this.request('getMe', {}, { signal })
  }

  async getUpdates({ offset = this.offset, timeout = 30, signal } = {}) {
    return this.request('getUpdates', { offset, timeout, allowed_updates: ['message'] }, { signal })
  }

  async sendMessage(chatId, text, options = {}) {
    const body = { chat_id: String(chatId), text: String(text || '').slice(0, 4096) }
    if (options.replyTo) body.reply_parameters = { message_id: Number(options.replyTo) }
    return this.request('sendMessage', body)
  }

  async sendDocument(chatId, file, options = {}) {
    const form = new FormData()
    form.set('chat_id', String(chatId))
    form.set(
      'document',
      file instanceof Blob
        ? file
        : new Blob([file], { type: options.mimeType || 'application/octet-stream' }),
      options.name || 'document',
    )
    return this.request('sendDocument', form, { multipart: true })
  }

  async sendPhoto(chatId, file, options = {}) {
    const form = new FormData()
    form.set('chat_id', String(chatId))
    form.set(
      'photo',
      file instanceof Blob ? file : new Blob([file], { type: options.mimeType || 'image/png' }),
      options.name || 'image',
    )
    return this.request('sendPhoto', form, { multipart: true })
  }

  async connect(config) {
    await this.disconnect()
    const token = validToken(config?.token)
    this.connection = { ...config, token }
    this.offset = Number.isInteger(config?.offset) ? config.offset : 0
    this.setStatus({ state: 'connecting', lastError: '', bot: null })
    try {
      const bot = await this.getMe()
      if (!bot?.id) throw new Error('Telegram getMe 未返回机器人身份。')
      this.controller = new AbortController()
      const controller = this.controller
      this.setStatus({
        state: 'connected',
        connectedAt: new Date().toISOString(),
        lastError: '',
        bot: {
          id: String(bot.id),
          name: bot.first_name || bot.username || 'Telegram Bot',
          username: bot.username || '',
        },
      })
      this.monitorPromise = this.monitor(controller.signal).catch((error) => {
        if (!controller.signal.aborted)
          this.setStatus({ state: 'failed', lastError: safeError(error) })
      })
      return this.getStatus()
    } catch (error) {
      this.setStatus({ state: 'failed', lastError: safeError(error) })
      this.connection = null
      throw error
    }
  }

  async monitor(signal) {
    let failures = 0
    while (!signal.aborted) {
      try {
        const updates = await this.getUpdates({ signal })
        if (signal.aborted) return
        failures = 0
        this.setStatus({ state: 'connected', lastEventAt: new Date().toISOString(), lastError: '' })
        const items = Array.isArray(updates) ? updates : []
        for (const update of items) {
          if (Number.isInteger(update.update_id)) this.offset = update.update_id + 1
          const message = this.mapMessage(update.message)
          if (message)
            Promise.resolve(this.onMessage(message)).catch((error) =>
              this.setStatus({ lastError: safeError(error) }),
            )
        }
        await abortableDelay(items.length ? 0 : 50, signal)
      } catch (error) {
        if (signal.aborted) return
        failures += 1
        this.setStatus({
          state: failures >= 3 ? 'reconnecting' : 'connected',
          lastError: safeError(error),
        })
        await abortableDelay(Math.min(30_000, failures * 2_000), signal)
      }
    }
  }

  mapMessage(raw) {
    if (!raw?.chat?.id || !raw.from?.id) return null
    const resources = []
    if (Array.isArray(raw.photo) && raw.photo.length) {
      const photo = raw.photo.at(-1)
      resources.push({
        type: 'image',
        fileId: photo.file_id,
        name: `telegram-${photo.file_id}.jpg`,
      })
    }
    if (raw.document?.file_id)
      resources.push({
        type: 'file',
        fileId: raw.document.file_id,
        name: raw.document.file_name || 'telegram-document',
      })
    return {
      messageId: String(raw.message_id || `${raw.chat.id}-${raw.date || Date.now()}`),
      peerId: String(raw.chat.id),
      senderId: String(raw.from.id),
      senderName:
        [raw.from.first_name, raw.from.last_name].filter(Boolean).join(' ') ||
        raw.from.username ||
        '',
      chatType: raw.chat.type === 'private' ? 'p2p' : 'group',
      content: String(raw.text || raw.caption || ''),
      resources,
    }
  }

  async send(message, input = {}) {
    if (!this.connection || !this.controller) throw new Error('Telegram 机器人尚未连接。')
    return this.sendMessage(message.peerId, input.markdown || input.text || '', {
      replyTo: message.messageId,
    })
  }

  async sendToPeer(peerId, input = {}) {
    if (!this.connection || !this.controller) throw new Error('Telegram 机器人尚未连接。')
    return this.sendMessage(peerId, input.markdown || input.text || '')
  }

  async sendAsset(peerId, asset) {
    if (!this.connection || !this.controller) throw new Error('Telegram 机器人尚未连接。')
    const data = await readFile(asset.path)
    return asset.mimeType?.startsWith('image/')
      ? this.sendPhoto(peerId, new Blob([data], { type: asset.mimeType }), asset)
      : this.sendDocument(
          peerId,
          new Blob([data], { type: asset.mimeType || 'application/octet-stream' }),
          asset,
        )
  }

  async downloadResources(resources = []) {
    const result = []
    let total = 0
    for (const resource of resources.slice(0, 8)) {
      if (!resource.fileId) continue
      const file = await this.request('getFile', { file_id: resource.fileId })
      if (!file?.file_path) continue
      const base = String(this.connection.fileBaseUrl || DEFAULT_FILE_BASE_URL).replace(/\/$/, '')
      let response
      try {
        response = await this.fetchImpl(
          `${base}/bot${validToken(this.connection.token)}/${file.file_path}`,
          {
            signal: this.controller?.signal,
          },
        )
      } catch (error) {
        throw new Error(`Telegram 附件下载请求失败：${safeError(error)}`)
      }
      if (!response.ok) throw new Error(`Telegram 附件下载失败（HTTP ${response.status}）。`)
      const buffer = Buffer.from(await response.arrayBuffer())
      total += buffer.length
      if (total > 24 * 1024 * 1024) throw new Error('Telegram 附件总大小超过 24 MB。')
      result.push({
        type: resource.type,
        name: resource.name,
        buffer,
        mimeType: response.headers.get('content-type') || '',
      })
    }
    return result
  }

  async disconnect() {
    const monitor = this.monitorPromise
    this.controller?.abort()
    this.controller = null
    this.monitorPromise = null
    this.connection = null
    await monitor?.catch(() => {})
    this.setStatus({ state: 'idle', connectedAt: null, bot: null })
  }
}

export { DEFAULT_BASE_URL as TELEGRAM_API_BASE }
