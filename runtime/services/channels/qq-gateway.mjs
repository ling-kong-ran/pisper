// QQ 官方机器人网关：封装鉴权、OpenAPI 请求与 WebSocket 事件循环。
// 将 HTTP API 与传输层分开，便于协议升级、替换代理以及在测试中诊断失败。
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
    if (typeof fetchImpl !== 'function') throw new Error('QQ 网关需要可用的 fetch 实现。')
    this.fetchImpl = fetchImpl
    this.baseUrl = String(baseUrl).replace(/\/$/, '')
    this.accessToken = accessToken
  }

  headers() {
    return { authorization: `QQBot ${this.accessToken}`, 'content-type': 'application/json' }
  }

  async request(url, options, label = '接口') {
    try {
      return await this.fetchImpl(url, options)
    } catch (error) {
      throw new Error(`QQ ${label} 请求失败：${safeError(error)}`)
    }
  }

  async parse(response, label) {
    let value
    try {
      value = await response.json()
    } catch {
      throw new Error(`QQ ${label} 返回了无效响应（HTTP ${response.status}）。`)
    }
    if (!response.ok) {
      const reason = value?.message || value?.msg || `HTTP ${response.status}`
      throw new Error(`QQ ${label} 失败：${String(reason).slice(0, 500)}`)
    }
    return value
  }

  async getAppAccessToken(appId, appSecret, signal) {
    if (!String(appId || '').trim() || !String(appSecret || '').trim())
      throw new Error('QQ App ID 与 App Secret 均不能为空。')
    const response = await this.request('https://bots.qq.com/app/getAppAccessToken', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ appId: String(appId), clientSecret: String(appSecret) }),
      signal,
    })
    const result = await this.parse(response, '获取访问令牌')
    if (!result?.access_token) throw new Error('QQ 鉴权响应缺少 access_token。')
    this.accessToken = result.access_token
    return result
  }

  async getGateway(signal) {
    const response = await this.request(`${this.baseUrl}/gateway`, {
      headers: this.headers(),
      signal,
    })
    const result = await this.parse(response, '获取 WebSocket 地址')
    if (!result?.url) throw new Error('QQ 网关响应缺少 WebSocket 地址。')
    return result.url
  }

  async getMe(signal) {
    const response = await this.request(`${this.baseUrl}/users/@me`, {
      headers: this.headers(),
      signal,
    })
    return this.parse(response, '获取机器人身份')
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
    return this.parse(response, '发送消息')
  }

  async sendPhoto(peer, file, signal) {
    return this.sendMessage(
      peer,
      { content: file?.url || '图片附件暂不支持直接上传，请使用图片 URL。' },
      signal,
    )
  }

  async sendDocument(peer, file, signal) {
    return this.sendMessage(peer, { content: file?.url || `文件：${file?.name || '附件'}` }, signal)
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
    if (!String(config?.appId || '').trim()) throw new Error('QQ App ID 不能为空。')
    if (!String(config?.appSecret || config?.token || '').trim())
      throw new Error('QQ App Secret/Token 不能为空。')
    this.controller = new AbortController()
    const signal = this.controller.signal
    this.setStatus({ state: 'connecting', lastError: '', bot: null })
    try {
      this.api = this.api || new QQBotApi({ fetchImpl: this.fetchImpl, baseUrl: config.baseUrl })
      if (config.appSecret && typeof this.api.getAppAccessToken === 'function')
        await this.api.getAppAccessToken(config.appId, config.appSecret, signal)
      else if (config.token) this.api.accessToken = String(config.token)
      else throw new Error('QQ 未提供可用的 App Secret/Token，无法鉴权。')
      const bot = await this.api.getMe(signal).catch(() => null)
      const gatewayUrl = await this.api.getGateway(signal)
      if (typeof this.WebSocketImpl !== 'function')
        throw new Error('当前运行环境没有 WebSocket，无法连接 QQ 官方机器人网关。')
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
        cleanup(reject, new Error(`QQ WebSocket 连接失败：${event?.message || '未知错误'}`))
      const onAbort = () => cleanup(reject, signal.reason || new Error('QQ 连接已停止。'))
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
        this.setStatus({ lastError: `QQ WebSocket 消息解析失败：${safeError(error)}` })
        return
      }
      this.handlePacket(packet, config)
    })
    addListener(socket, 'close', () => {
      clearInterval(this.heartbeatTimer)
      this.heartbeatTimer = null
      if (!signal.aborted) {
        this.setStatus({ state: 'reconnecting', lastError: 'QQ WebSocket 已断开。' })
        this.scheduleReconnect(config, signal)
      }
    })
    addListener(socket, 'error', (error) => {
      if (!signal.aborted) this.setStatus({ lastError: `QQ WebSocket 错误：${safeError(error)}` })
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
          this.setStatus({ state: 'failed', lastError: `QQ 重连失败：${safeError(error)}` })
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
      this.setStatus({ state: 'reconnecting', lastError: 'QQ 网关要求重新连接。' })
      this.socket?.close()
      return
    }
    if (packet.op === 9) {
      this.setStatus({
        state: 'failed',
        lastError: 'QQ 网关鉴权失败：请检查 App ID、App Secret/Token 与 intents 配置。',
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
      content: String(raw.content || '').trim(),
      resources: [],
    }
  }

  async send(message, input = {}) {
    if (!this.api || !this.controller || !this.socket) throw new Error('QQ 机器人尚未连接。')
    return this.api.sendMessage(
      { peerId: message.peerId, chatType: message.chatType },
      { content: input.markdown || input.text, replyTo: message.messageId },
      this.controller.signal,
    )
  }

  async sendToPeer(peerId, input = {}, scope = {}) {
    if (!this.api || !this.controller) throw new Error('QQ 机器人尚未连接。')
    return this.api.sendMessage(
      { peerId, chatType: scope.chatType || 'p2p' },
      { content: input.markdown || input.text },
      this.controller.signal,
    )
  }

  async sendAsset(peerId, asset, scope = {}) {
    if (!this.api || !this.controller) throw new Error('QQ 机器人尚未连接。')
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
