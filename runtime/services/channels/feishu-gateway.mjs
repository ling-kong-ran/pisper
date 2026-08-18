// 飞书网关：封装飞书长连接（WebSocket）接入、消息收发与连接状态管理。
// SDK 懒加载：@larksuiteoapi/node-sdk 体积大，仅在飞书渠道接入时引入。
// Lazily loaded: @larksuiteoapi/node-sdk is large and only needed once a Feishu channel connects.
const larkSdk = () => import('@larksuiteoapi/node-sdk')

export class FeishuGateway {
  constructor({ createChannelImpl = null, onMessage, onStatusChange = () => {} }) {
    this.createChannelImpl = createChannelImpl
    this.onMessage = onMessage
    this.onStatusChange = onStatusChange
    this.channel = null
    this.status = { state: 'idle', lastError: '', connectedAt: null }
  }

  setStatus(patch) {
    this.status = { ...this.status, ...patch }
    this.onStatusChange(this.getStatus())
  }

  getStatus() {
    const live = this.channel?.getConnectionStatus?.()
    return {
      ...this.status,
      state: live?.state || this.status.state,
      reconnectAttempts: live?.reconnectAttempts || 0,
      lastConnectTime: live?.lastConnectTime || null,
      bot: this.channel?.botIdentity || null,
    }
  }

  async connect(config) {
    await this.disconnect()
    this.setStatus({ state: 'connecting', lastError: '' })
    const { createLarkChannel, Domain, LoggerLevel } = await larkSdk()
    const createChannel = this.createChannelImpl || createLarkChannel
    const channel = createChannel({
      appId: config.appId,
      appSecret: config.appSecret,
      domain: config.domain === 'lark' ? Domain.Lark : Domain.Feishu,
      transport: 'websocket',
      source: 'pisper',
      loggerLevel: LoggerLevel.warn,
      handshakeTimeoutMs: 15_000,
      safety: { chatQueue: { enabled: false }, staleMessageWindowMs: 5 * 60 * 1000 },
      policy: { dmMode: 'open', requireMention: true, respondToMentionAll: false },
      outbound: { textChunkLimit: 3500 },
    })
    this.channel = channel
    channel.on({
      message: (message) => {
        Promise.resolve(this.onMessage({ ...message, peerId: message.chatId })).catch((error) => {
          this.setStatus({ lastError: error instanceof Error ? error.message : String(error) })
        })
      },
      error: (error) => this.setStatus({ lastError: error.message || String(error) }),
      reconnecting: () => this.setStatus({ state: 'reconnecting' }),
      reconnected: () => this.setStatus({ state: 'connected', lastError: '' }),
    })
    try {
      await channel.connect()
      this.setStatus({ state: 'connected', connectedAt: new Date().toISOString(), lastError: '' })
      return this.getStatus()
    } catch (error) {
      this.setStatus({
        state: 'failed',
        lastError: error instanceof Error ? error.message : String(error),
      })
      throw error
    }
  }

  async disconnect() {
    const current = this.channel
    this.channel = null
    if (current) await current.disconnect().catch(() => {})
    this.setStatus({ state: 'idle', connectedAt: null })
  }

  async send(message, input) {
    if (!this.channel) throw new Error('飞书机器人尚未连接。')
    return this.channel.send(message.peerId, input, { replyTo: message.messageId })
  }

  async sendToPeer(peerId, input) {
    if (!this.channel) throw new Error('飞书机器人尚未连接。')
    return this.channel.send(peerId, input)
  }

  async sendAsset(peerId, asset) {
    if (!this.channel) throw new Error('飞书机器人尚未连接。')
    const input = asset.mimeType?.startsWith('image/')
      ? { image: { source: asset.path } }
      : asset.mimeType?.startsWith('video/')
        ? { video: { source: asset.path } }
        : { file: { source: asset.path, fileName: asset.name } }
    return this.channel.send(peerId, input)
  }

  async downloadResources(resources = []) {
    if (!this.channel) throw new Error('飞书机器人尚未连接。')
    const result = []
    let total = 0
    for (const resource of resources.slice(0, 8)) {
      const kind = resource.type === 'image' ? 'image' : 'file'
      const buffer = await this.channel.downloadResource(resource.fileKey, kind)
      total += buffer.length
      if (total > 24 * 1024 * 1024) throw new Error('附件总大小超过 24 MB。')
      result.push({
        type: resource.type,
        name: resource.fileName || `${resource.type}-${result.length + 1}`,
        buffer,
      })
    }
    return result
  }
}
