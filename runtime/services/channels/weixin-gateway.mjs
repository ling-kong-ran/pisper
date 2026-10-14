// 微信网关：基于微信开放平台协议的机器人接入（同步缓冲区轮询 + 媒体收发），
// 与飞书网关对等，供 weixin 渠道使用。
import { WeixinProtocol, weixinMediaItems, weixinTextFromItems } from './weixin-protocol.mjs'

function abortableDelay(ms, signal) {
  if (signal.aborted) return Promise.resolve()
  return new Promise((resolve) => {
    const timer = setTimeout(done, ms)
    function done() {
      clearTimeout(timer)
      signal.removeEventListener('abort', done)
      resolve()
    }
    signal.addEventListener('abort', done, { once: true })
  })
}

export class WeixinGateway {
  constructor({
    protocol = new WeixinProtocol(),
    onMessage,
    onSyncBuf = () => {},
    onStatusChange = () => {},
  }) {
    this.protocol = protocol
    this.onMessage = onMessage
    this.onSyncBuf = onSyncBuf
    this.onStatusChange = onStatusChange
    this.connection = null
    this.controller = null
    this.monitorPromise = null
    this.status = { state: 'idle', lastError: '', connectedAt: null, lastEventAt: null }
  }

  setStatus(patch) {
    this.status = { ...this.status, ...patch }
    this.onStatusChange(this.getStatus())
  }

  getStatus() {
    return { ...this.status }
  }

  async connect(connection) {
    await this.disconnect()
    this.connection = connection
    this.controller = new AbortController()
    this.setStatus({ state: 'connecting', lastError: '' })
    try {
      const started = await this.protocol.notifyStart(connection)
      if (started.ret && started.ret !== 0)
        throw new Error(started.errmsg || `微信连接失败（${started.ret}）`)
      const controller = this.controller
      this.setStatus({ state: 'connected', connectedAt: new Date().toISOString(), lastError: '' })
      this.monitorPromise = this.monitor(controller.signal).catch((error) => {
        if (!controller.signal.aborted)
          this.setStatus({
            state: 'failed',
            lastError: error instanceof Error ? error.message : String(error),
          })
      })
      return this.getStatus()
    } catch (error) {
      this.setStatus({
        state: 'failed',
        lastError: error instanceof Error ? error.message : String(error),
      })
      throw error
    }
  }

  async monitor(signal) {
    let syncBuf = this.connection.syncBuf || ''
    let timeout = 35_000
    let failures = 0
    while (!signal.aborted) {
      try {
        const result = await this.protocol.getUpdates(this.connection, syncBuf, signal, timeout)
        if (signal.aborted) return
        if ((result.ret && result.ret !== 0) || (result.errcode && result.errcode !== 0))
          throw new Error(result.errmsg || `微信同步失败（${result.errcode || result.ret}）`)
        failures = 0
        this.setStatus({ state: 'connected', lastEventAt: new Date().toISOString(), lastError: '' })
        if (result.longpolling_timeout_ms > 0) timeout = result.longpolling_timeout_ms
        if (result.get_updates_buf && result.get_updates_buf !== syncBuf) {
          syncBuf = result.get_updates_buf
          this.connection.syncBuf = syncBuf
          this.onSyncBuf(syncBuf)
        }
        for (const raw of result.msgs || []) {
          if (raw.message_type === 2 || !raw.from_user_id) continue
          const message = {
            messageId: String(
              raw.message_id ||
                raw.client_id ||
                `${raw.from_user_id}-${raw.create_time_ms || Date.now()}`,
            ),
            peerId: raw.from_user_id,
            senderId: raw.from_user_id,
            senderName: '',
            chatType: 'p2p',
            content: weixinTextFromItems(raw.item_list),
            resources: weixinMediaItems(raw.item_list),
            contextToken: raw.context_token || '',
          }
          this.onMessage(message)
        }
      } catch (error) {
        if (signal.aborted) return
        failures += 1
        this.setStatus({
          state: failures >= 3 ? 'reconnecting' : 'connected',
          lastError: error instanceof Error ? error.message : String(error),
        })
        await abortableDelay(failures >= 3 ? 30_000 : 2_000, signal)
      }
    }
  }

  async disconnect() {
    const connection = this.connection
    const monitorPromise = this.monitorPromise
    this.controller?.abort()
    this.controller = null
    this.monitorPromise = null
    this.connection = null
    await monitorPromise?.catch(() => {})
    if (connection) await this.protocol.notifyStop(connection).catch(() => {})
    this.setStatus({ state: 'idle', connectedAt: null })
  }

  send(message, input) {
    const text = input.markdown || input.text || ''
    return this.protocol.sendText(this.connection, {
      to: message.peerId,
      text,
      contextToken: message.contextToken,
    })
  }

  sendToPeer(peerId, input, scope = {}) {
    // Tencent iLink requires the most recently issued inbound context_token
    // for both replies and proactive/cron delivery.
    if (input.path)
      return this.protocol.sendMedia(this.connection, {
        to: peerId,
        path: input.path,
        name: input.name,
        mimeType: input.mimeType,
        contextToken: scope.contextToken,
      })
    return this.protocol.sendText(this.connection, {
      to: peerId,
      text: input.markdown || input.text || '',
      contextToken: scope.contextToken,
    })
  }

  sendAsset(peerId, asset, scope = {}) {
    return this.protocol.sendMedia(this.connection, {
      to: peerId,
      path: asset.path,
      name: asset.name,
      mimeType: asset.mimeType,
      contextToken: scope.contextToken,
    })
  }

  async downloadResources(resources = []) {
    const result = []
    let total = 0
    for (const item of resources.slice(0, 8)) {
      const resource = await this.protocol.downloadItem(this.connection, item)
      if (!resource) continue
      total += resource.buffer.length
      if (total > 24 * 1024 * 1024) throw new Error('微信附件总大小超过 24 MB。')
      result.push(resource)
    }
    return result
  }
}
