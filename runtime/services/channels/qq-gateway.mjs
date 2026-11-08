// QQ 官方机器人网关：封装鉴权、OpenAPI 请求与 WebSocket 事件循环。
// 将 HTTP API 与传输层分开，便于协议升级、替换代理以及在测试中诊断失败。
import { pisperError } from '../../../shared/errors.mjs'
import { CHAT_REPLIES, qqFileText } from './chat-replies.mjs'

const DEFAULT_API_BASE = 'https://api.sgroup.qq.com'

function safeError(error) {
  return (error instanceof Error ? error.message : String(error))
    .replace(/(Bot\s+)?\d+\.[^\s)]+/gi, 'Bot ***')
    .slice(0, 1000)
}

function addListener(socket, event, listener) {
  if (typeof socket.addEventListener === 'function') socket.addEventListener(event, listener)
  else if (typeof socket.on === 'function') socket.on(event, listener)
  else socket[`on${event}`] = listener
}

function removeListener(socket, event, listener) {
  if (typeof socket.removeEventListener === 'function') socket.removeEventListener(event, listener)
  else if (typeof socket.off === 'function') socket.off(event, listener)
}

export class QQBotApi {
  constructor({ fetchImpl = globalThis.fetch, baseUrl = DEFAULT_API_BASE, accessToken = '' } = {}) {
    if (typeof fetchImpl !== 'function')
      throw pisperError('unavailable', 'network', 'QQ gateway needs fetch')
    this.fetchImpl = fetchImpl
    this.baseUrl = String(baseUrl).replace(/\/$/, '')
    this.accessToken = accessToken
  }

  headers() {
    return { authorization: `QQBot ${this.accessToken}`, 'content-type': 'application/json' }
  }

  async request(url, options, label = 'API') {
    try {
      return await this.fetchImpl(url, options)
    } catch (error) {
      throw pisperError('failed', 'network', `QQ ${label}: ${safeError(error)}`)
    }
  }

  async parse(response, label) {
    let value
    try {
      value = await response.json()
    } catch {
      throw pisperError(
        'failed',
        'channel',
        `QQ ${label}: invalid response (HTTP ${response.status})`,
      )
    }
    if (!response.ok) {
      const reason = value?.message || value?.msg || `HTTP ${response.status}`
      throw pisperError('failed', 'channel', `QQ ${label}: ${String(reason).slice(0, 500)}`)
    }
    return value
  }

  async getAppAccessToken(appId, appSecret, signal) {
    if (!String(appId || '').trim() || !String(appSecret || '').trim())
      throw pisperError('invalid', 'api_key', 'QQ App ID and App Secret are required')
    const response = await this.request('https://bots.qq.com/app/getAppAccessToken', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ appId: String(appId), clientSecret: String(appSecret) }),
      signal,
    })
    const result = await this.parse(response, 'access token')
    if (!result?.access_token)
      throw pisperError('failed', 'login', 'QQ auth response has no access_token')
    this.accessToken = result.access_token
    return result
  }

  async getGateway(signal) {
    const response = await this.request(`${this.baseUrl}/gateway`, {
      headers: this.headers(),
      signal,
    })
    const result = await this.parse(response, 'gateway URL')
    if (!result?.url)
      throw pisperError('failed', 'channel', 'QQ gateway response has no WebSocket URL')
    return result.url
  }

  async getMe(signal) {
    const response = await this.request(`${this.baseUrl}/users/@me`, {
      headers: this.headers(),
      signal,
    })
    return this.parse(response, 'bot profile')
  }

  async sendMessage(peer, input, signal) {
    const kind =
      peer.chatType === 'channel' ? 'channels' : peer.chatType === 'group' ? 'groups' : 'users'
    const id = encodeURIComponent(String(peer.peerId || peer.id || ''))
    const response = await this.request(`${this.baseUrl}/v2/${kind}/${id}/messages`, {
      method: 'POST',
      headers: this.headers(),
      body: JSON.stringify({
        content: String(input.content || input.text || '').slice(0, 4000),
        msg_type: 0,
        msg_id: input.replyTo ? String(input.replyTo) : undefined,
      }),
      signal,
    })
    return this.parse(response, 'send message')
  }

  async sendPhoto(peer, file, signal) {
    return this.sendMessage(peer, { content: file?.url || CHAT_REPLIES.qqPhotoUnsupported }, signal)
  }

  async sendDocument(peer, file, signal) {
    return this.sendMessage(peer, { content: file?.url || qqFileText(file?.name) }, signal)
  }
}

export class QQGateway {
  constructor({
    api = null,
    fetchImpl = globalThis.fetch,
    WebSocketImpl = globalThis.WebSocket,
    onMessage = () => {},
    onStatusChange = () => {},
  } = {}) {
    this.api = api
    this.fetchImpl = fetchImpl
    this.WebSocketImpl = WebSocketImpl
    this.onMessage = onMessage
    this.onStatusChange = onStatusChange
    this.socket = null
    this.controller = null
    this.heartbeatTimer = null
    this.reconnectTimer = null
    this.status = { state: 'idle', lastError: '', connectedAt: null, lastEventAt: null, bot: null }
    this.sequence = null
  }

  setStatus(patch) {
    this.status = { ...this.status, ...patch }
    this.onStatusChange(this.getStatus())
  }

  getStatus() {
    return { ...this.status }
  }

  async connect(config) {
    await this.disconnect()
    if (!String(config?.appId || '').trim())
      throw pisperError('invalid', 'api_key', 'QQ App ID is required')
    if (!String(config?.appSecret || config?.token || '').trim())
      throw pisperError('invalid', 'api_key', 'QQ App Secret/Token is required')
    this.controller = new AbortController()
    const signal = this.controller.signal
    this.setStatus({ state: 'connecting', lastError: '', bot: null })
    try {
      this.api = this.api || new QQBotApi({ fetchImpl: this.fetchImpl, baseUrl: config.baseUrl })
      if (config.appSecret && typeof this.api.getAppAccessToken === 'function')
        await this.api.getAppAccessToken(config.appId, config.appSecret, signal)
      else if (config.token) this.api.accessToken = String(config.token)
      else throw pisperError('invalid', 'api_key', 'QQ App Secret/Token is required')
      const bot = await this.api.getMe(signal).catch(() => null)
      const gatewayUrl = await this.api.getGateway(signal)
      if (typeof this.WebSocketImpl !== 'function')
        throw pisperError('unsupported', 'channel', 'no WebSocket in this environment for QQ')
      const socket = new this.WebSocketImpl(gatewayUrl)
      this.socket = socket
      await this.waitOpen(socket, signal)
      this.setStatus({
        state: 'connected',
        connectedAt: new Date().toISOString(),
        lastError: '',
        bot: bot || null,
      })
      this.bindSocket(socket, config, signal)
      return this.getStatus()
    } catch (error) {
      const message = safeError(error)
      await this.disconnect()
      this.setStatus({ state: 'failed', lastError: message })
      throw error
    }
  }

  waitOpen(socket, signal) {
    if (socket.readyState === 1) return Promise.resolve()
    return new Promise((resolve, reject) => {
      const onOpen = () => cleanup(resolve)
      const onError = (event) =>
        cleanup(
          reject,
          pisperError('failed', 'network', `QQ WebSocket: ${event?.message || 'unknown error'}`),
        )
      const onAbort = () =>
        cleanup(reject, signal.reason || pisperError('cancelled', 'channel', 'QQ connection'))
      const cleanup = (callback, value) => {
        removeListener(socket, 'open', onOpen)
        removeListener(socket, 'error', onError)
        signal.removeEventListener('abort', onAbort)
        callback(value)
      }
      addListener(socket, 'open', onOpen)
      addListener(socket, 'error', onError)
      signal.addEventListener('abort', onAbort, { once: true })
    })
  }

  bindSocket(socket, config, signal) {
    addListener(socket, 'message', (event) => {
      let packet
      try {
        const value = event?.data ?? event
        packet = JSON.parse(typeof value === 'string' ? value : Buffer.from(value).toString('utf8'))
      } catch (error) {
        this.setStatus({
          lastError: `QQ WebSocket message could not be parsed: ${safeError(error)}`,
        })
        return
      }
      this.handlePacket(packet, config)
    })
    addListener(socket, 'close', () => {
      clearInterval(this.heartbeatTimer)
      this.heartbeatTimer = null
      if (!signal.aborted) {
        this.setStatus({ state: 'reconnecting', lastError: 'QQ WebSocket disconnected.' })
        this.scheduleReconnect(config, signal)
      }
    })
    addListener(socket, 'error', (error) => {
      if (!signal.aborted) this.setStatus({ lastError: `QQ WebSocket error: ${safeError(error)}` })
    })
  }

  scheduleReconnect(config, signal) {
    if (this.reconnectTimer || signal.aborted) return
    this.reconnectTimer = setTimeout(async () => {
      this.reconnectTimer = null
      if (signal.aborted) return
      try {
        await this.connect(config)
      } catch (error) {
        if (!signal.aborted)
          this.setStatus({ state: 'failed', lastError: `QQ reconnect failed: ${safeError(error)}` })
      }
    }, 1_000)
  }

  handlePacket(packet, config = {}) {
    if (packet.s !== undefined && packet.s !== null) this.sequence = packet.s
    if (packet.op === 10) {
      const interval = Math.max(5_000, Number(packet.d?.heartbeat_interval) || 41_250)
      clearInterval(this.heartbeatTimer)
      this.heartbeatTimer = setInterval(() => {
        if (this.socket?.readyState === 1)
          this.socket.send(JSON.stringify({ op: 1, d: this.sequence }))
      }, interval)
      this.socket?.send(
        JSON.stringify({
          op: 2,
          d: {
            token: `QQBot ${this.api.accessToken}`,
            // 默认订阅私聊、群聊 @ 与频道 @ 事件，避免上线后只能保持连接却收不到消息。
            intents: Number(config.intents) || (1 << 25) | (1 << 26) | (1 << 30),
            shard: [0, 1],
            properties: { $os: process.platform, $browser: 'pisper', $device: 'pisper' },
          },
        }),
      )
      return
    }
    if (packet.op === 7) {
      this.setStatus({ state: 'reconnecting', lastError: 'QQ gateway asked to reconnect.' })
      this.socket?.close()
      return
    }
    if (packet.op === 9) {
      this.setStatus({
        state: 'failed',
        lastError: 'QQ gateway sign-in failed: check App ID, App Secret/Token and intents.',
      })
      return
    }
    if (packet.op !== 0) return
    const message = this.mapMessage(packet.t, packet.d)
    if (message) {
      this.setStatus({ state: 'connected', lastEventAt: new Date().toISOString(), lastError: '' })
      Promise.resolve(this.onMessage(message)).catch((error) =>
        this.setStatus({ lastError: safeError(error) }),
      )
    }
  }

  mapMessage(type, raw) {
    if (
      !raw ||
      !['C2C_MESSAGE_CREATE', 'GROUP_AT_MESSAGE_CREATE', 'AT_MESSAGE_CREATE'].includes(type)
    )
      return null
    const group = type !== 'C2C_MESSAGE_CREATE'
    const peerId = group
      ? raw.group_openid || raw.group_id || raw.channel_id
      : raw.author?.user_openid || raw.author?.id
    const senderId = raw.author?.user_openid || raw.author?.id
    if (!peerId || !senderId) return null
    return {
      messageId: String(raw.id || `${senderId}-${Date.now()}`),
      peerId: String(peerId),
      senderId: String(senderId),
      senderName: raw.author?.member_openid || raw.author?.username || '',
      chatType: type === 'AT_MESSAGE_CREATE' ? 'channel' : group ? 'group' : 'p2p',
      // QQ 只投递 @ 了机器人的群消息，内容里已经去掉了 @：标上平台无关的 mentioned，分派时照样算点名。
      ...(group ? { mentioned: true } : {}),
      content: String(raw.content || '').trim(),
      resources: [],
    }
  }

  async send(message, input = {}) {
    if (!this.api || !this.controller || !this.socket)
      throw pisperError('unavailable', 'channel', 'QQ bot is not connected')
    return this.api.sendMessage(
      { peerId: message.peerId, chatType: message.chatType },
      { content: input.markdown || input.text, replyTo: message.messageId },
      this.controller.signal,
    )
  }

  async sendToPeer(peerId, input = {}, scope = {}) {
    if (!this.api || !this.controller)
      throw pisperError('unavailable', 'channel', 'QQ bot is not connected')
    return this.api.sendMessage(
      { peerId, chatType: scope.chatType || 'p2p' },
      { content: input.markdown || input.text },
      this.controller.signal,
    )
  }

  async sendAsset(peerId, asset, scope = {}) {
    if (!this.api || !this.controller)
      throw pisperError('unavailable', 'channel', 'QQ bot is not connected')
    const peer = { peerId, chatType: scope.chatType || 'p2p' }
    return asset.mimeType?.startsWith('image/')
      ? this.api.sendPhoto(peer, asset, this.controller.signal)
      : this.api.sendDocument(peer, asset, this.controller.signal)
  }

  async downloadResources() {
    return []
  }

  async disconnect() {
    this.controller?.abort()
    this.controller = null
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
    clearInterval(this.heartbeatTimer)
    this.heartbeatTimer = null
    const socket = this.socket
    this.socket = null
    try {
      if (socket && socket.readyState === 1) socket.close()
    } catch {}
    this.setStatus({ state: 'idle', connectedAt: null, bot: null })
  }
}

export { QQGateway as QqGateway, DEFAULT_API_BASE as QQ_API_BASE }
