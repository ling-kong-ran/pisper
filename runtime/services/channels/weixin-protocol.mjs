// 微信协议层：消息编解码、媒体加密传输、同步缓冲区协议。
// 协议形状与媒体加解密遵循腾讯 MIT 开源实现 @tencent-weixin/openclaw-weixin 2.4.6。
// Protocol shapes and media crypto follow Tencent's MIT-licensed
// @tencent-weixin/openclaw-weixin 2.4.6 implementation.
import { createCipheriv, createDecipheriv, createHash, randomBytes, randomUUID } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { basename, extname } from 'node:path'

export const WEIXIN_API_BASE = 'https://ilinkai.weixin.qq.com'
export const WEIXIN_CDN_BASE = 'https://novac2c.cdn.weixin.qq.com/c2c'
const CHANNEL_VERSION = '2.4.6'
const CLIENT_VERSION = (2 << 16) | (4 << 8) | 6
const MESSAGE_ITEM = { TEXT: 1, IMAGE: 2, VOICE: 3, FILE: 4, VIDEO: 5 }
const UPLOAD_MEDIA = { IMAGE: 1, VIDEO: 2, FILE: 3 }

function baseInfo() {
  return { channel_version: CHANNEL_VERSION, bot_agent: 'Pisper/0.0.0' }
}

function commonHeaders() {
  return { 'iLink-App-Id': 'bot', 'iLink-App-ClientVersion': String(CLIENT_VERSION) }
}

function authenticatedHeaders(token) {
  const uin = randomBytes(4).readUInt32BE(0)
  const headers = {
    ...commonHeaders(),
    'Content-Type': 'application/json',
    AuthorizationType: 'ilink_bot_token',
    'X-WECHAT-UIN': Buffer.from(String(uin)).toString('base64'),
  }
  if (String(token || '').trim()) headers.Authorization = `Bearer ${String(token).trim()}`
  return headers
}

function requestSignal(timeoutMs, external) {
  const signals = []
  if (timeoutMs) signals.push(AbortSignal.timeout(timeoutMs))
  if (external) signals.push(external)
  return signals.length > 1 ? AbortSignal.any(signals) : signals[0]
}

async function responseJson(response, label) {
  const text = await response.text()
  if (!response.ok)
    throw new Error(`${label} 请求失败（HTTP ${response.status}）：${text.slice(0, 300)}`)
  try {
    return text ? JSON.parse(text) : {}
  } catch {
    throw new Error(`${label} 返回了无法解析的数据。`)
  }
}

function parseAesKey(value) {
  const decoded = Buffer.from(String(value || ''), 'base64')
  if (decoded.length === 16) return decoded
  if (decoded.length === 32 && /^[0-9a-f]{32}$/i.test(decoded.toString('ascii')))
    return Buffer.from(decoded.toString('ascii'), 'hex')
  throw new Error('微信媒体 AES 密钥格式不正确。')
}

function decryptAesEcb(ciphertext, key) {
  const decipher = createDecipheriv('aes-128-ecb', key, null)
  return Buffer.concat([decipher.update(ciphertext), decipher.final()])
}

function encryptAesEcb(plaintext, key) {
  const cipher = createCipheriv('aes-128-ecb', key, null)
  return Buffer.concat([cipher.update(plaintext), cipher.final()])
}

function paddedSize(size) {
  return Math.ceil((size + 1) / 16) * 16
}

function mimeFromName(name) {
  return (
    {
      '.png': 'image/png',
      '.jpg': 'image/jpeg',
      '.jpeg': 'image/jpeg',
      '.gif': 'image/gif',
      '.webp': 'image/webp',
      '.bmp': 'image/bmp',
      '.mp4': 'video/mp4',
      '.mov': 'video/quicktime',
      '.webm': 'video/webm',
      '.pdf': 'application/pdf',
      '.docx': 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
      '.pptx': 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
      '.xlsx': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
      '.txt': 'text/plain',
      '.md': 'text/markdown',
      '.json': 'application/json',
      '.csv': 'text/csv',
    }[extname(String(name || '')).toLowerCase()] || 'application/octet-stream'
  )
}

function mediaTypeFor(mimeType) {
  if (mimeType.startsWith('image/')) return UPLOAD_MEDIA.IMAGE
  if (mimeType.startsWith('video/')) return UPLOAD_MEDIA.VIDEO
  return UPLOAD_MEDIA.FILE
}

export class WeixinProtocol {
  constructor({ fetchImpl = globalThis.fetch } = {}) {
    this.fetchImpl = fetchImpl
  }

  async request(
    url,
    {
      method = 'POST',
      token,
      body,
      timeoutMs = 15_000,
      signal,
      authenticated = true,
      label = '微信接口',
    } = {},
  ) {
    const response = await this.fetchImpl(url, {
      method,
      headers: authenticated ? authenticatedHeaders(token) : commonHeaders(),
      ...(body == null ? {} : { body: JSON.stringify(body) }),
      signal: requestSignal(timeoutMs, signal),
    })
    return responseJson(response, label)
  }

  startQr({ localTokens = [] } = {}) {
    return this.request(`${WEIXIN_API_BASE}/ilink/bot/get_bot_qrcode?bot_type=3`, {
      body: { local_token_list: localTokens.slice(-10) },
      timeoutMs: 0,
      label: '获取微信登录二维码',
    })
  }

  async pollQr({ qrcode, baseUrl = WEIXIN_API_BASE, verifyCode, signal }) {
    const query = new URLSearchParams({ qrcode })
    if (verifyCode) query.set('verify_code', verifyCode)
    try {
      return await this.request(`${baseUrl}/ilink/bot/get_qrcode_status?${query}`, {
        method: 'GET',
        authenticated: false,
        timeoutMs: 35_000,
        signal,
        label: '查询微信扫码状态',
      })
    } catch (error) {
      if (!signal?.aborted && (error?.name === 'TimeoutError' || error?.name === 'AbortError'))
        return { status: 'wait' }
      throw error
    }
  }

  async notifyStart(connection) {
    const result = await this.authPost(connection, 'ilink/bot/msg/notifystart', {})
    if (result.ret && result.ret !== 0)
      throw new Error(result.errmsg || `微信连接失败（${result.ret}）`)
    return result
  }

  async notifyStop(connection) {
    return this.authPost(connection, 'ilink/bot/msg/notifystop', {}, { timeoutMs: 8_000 })
  }

  async getUpdates(connection, syncBuf, signal, timeoutMs = 35_000) {
    try {
      return await this.authPost(
        connection,
        'ilink/bot/getupdates',
        { get_updates_buf: syncBuf || '' },
        { timeoutMs, signal },
      )
    } catch (error) {
      if (signal?.aborted || error?.name === 'TimeoutError' || error?.name === 'AbortError')
        return { ret: 0, msgs: [], get_updates_buf: syncBuf }
      throw error
    }
  }

  authPost(connection, endpoint, body, options = {}) {
    const baseUrl = String(connection.baseUrl || WEIXIN_API_BASE).replace(/\/$/, '')
    return this.request(`${baseUrl}/${endpoint}`, {
      token: connection.token,
      body: { ...body, base_info: baseInfo() },
      timeoutMs: options.timeoutMs ?? 15_000,
      signal: options.signal,
      label: options.label || '微信 iLink',
    })
  }

  async sendText(connection, { to, text, contextToken }) {
    const clientId = `pisper-${randomUUID()}`
    const result = await this.authPost(connection, 'ilink/bot/sendmessage', {
      msg: {
        from_user_id: '',
        to_user_id: to,
        client_id: clientId,
        message_type: 2,
        message_state: 2,
        item_list: [{ type: MESSAGE_ITEM.TEXT, text_item: { text: String(text || '') } }],
        context_token: contextToken || undefined,
      },
    })
    if (result.ret && result.ret !== 0) {
      if (result.ret === -2 && result.errmsg === 'prepare failed')
        throw new Error('微信会话上下文已失效，请先在微信中向机器人发送一条消息后重试。')
      throw new Error(result.errmsg || `微信消息发送失败（${result.ret}）`)
    }
    return { messageId: clientId }
  }

  async downloadItem(connection, item) {
    const cdnBase = String(connection.cdnBaseUrl || WEIXIN_CDN_BASE).replace(/\/$/, '')
    let media
    let name
    let mimeType
    let rawAesKey
    if (item.type === MESSAGE_ITEM.IMAGE) {
      media = item.image_item?.media
      rawAesKey = item.image_item?.aeskey ? Buffer.from(item.image_item.aeskey, 'hex') : null
      name = `weixin-image-${item.msg_id || Date.now()}.png`
      mimeType = 'image/png'
    } else if (item.type === MESSAGE_ITEM.FILE) {
      media = item.file_item?.media
      name = item.file_item?.file_name || `weixin-file-${Date.now()}`
      mimeType = mimeFromName(name)
    } else if (item.type === MESSAGE_ITEM.VIDEO) {
      media = item.video_item?.media
      name = `weixin-video-${item.msg_id || Date.now()}.mp4`
      mimeType = 'video/mp4'
    } else return null
    const url =
      media?.full_url ||
      (media?.encrypt_query_param
        ? `${cdnBase}/download?encrypted_query_param=${encodeURIComponent(media.encrypt_query_param)}`
        : '')
    if (!url) return null
    const response = await this.fetchImpl(url, { signal: AbortSignal.timeout(30_000) })
    if (!response.ok) throw new Error(`微信附件下载失败（HTTP ${response.status}）`)
    const encrypted = Buffer.from(await response.arrayBuffer())
    const key = rawAesKey || (media?.aes_key ? parseAesKey(media.aes_key) : null)
    const buffer = key ? decryptAesEcb(encrypted, key) : encrypted
    return {
      type: mimeType.startsWith('image/')
        ? 'image'
        : mimeType.startsWith('video/')
          ? 'video'
          : 'file',
      name,
      mimeType,
      buffer,
    }
  }

  async sendMedia(connection, { to, path, name, mimeType, contextToken }) {
    const plaintext = await readFile(path)
    const key = randomBytes(16)
    const filekey = randomBytes(16).toString('hex')
    const resolvedMime = mimeType || mimeFromName(name || path)
    const result = await this.authPost(connection, 'ilink/bot/getuploadurl', {
      filekey,
      media_type: mediaTypeFor(resolvedMime),
      to_user_id: to,
      rawsize: plaintext.length,
      rawfilemd5: createHash('md5').update(plaintext).digest('hex'),
      filesize: paddedSize(plaintext.length),
      no_need_thumb: true,
      aeskey: key.toString('hex'),
    })
    const uploadUrl =
      result.upload_full_url ||
      (result.upload_param
        ? `${String(connection.cdnBaseUrl || WEIXIN_CDN_BASE).replace(/\/$/, '')}/upload?encrypted_query_param=${encodeURIComponent(result.upload_param)}&filekey=${filekey}`
        : '')
    if (!uploadUrl) throw new Error('微信媒体上传地址为空。')
    const upload = await this.fetchImpl(uploadUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/octet-stream' },
      body: new Uint8Array(encryptAesEcb(plaintext, key)),
      signal: AbortSignal.timeout(60_000),
    })
    if (!upload.ok) throw new Error(`微信媒体上传失败（HTTP ${upload.status}）`)
    const downloadParam = upload.headers.get('x-encrypted-param')
    if (!downloadParam) throw new Error('微信媒体上传响应缺少下载参数。')
    const media = {
      encrypt_query_param: downloadParam,
      aes_key: Buffer.from(key.toString('hex')).toString('base64'),
      encrypt_type: 1,
    }
    const type = resolvedMime.startsWith('image/')
      ? MESSAGE_ITEM.IMAGE
      : resolvedMime.startsWith('video/')
        ? MESSAGE_ITEM.VIDEO
        : MESSAGE_ITEM.FILE
    const item =
      type === MESSAGE_ITEM.IMAGE
        ? { type, image_item: { media, mid_size: paddedSize(plaintext.length) } }
        : type === MESSAGE_ITEM.VIDEO
          ? { type, video_item: { media, video_size: paddedSize(plaintext.length) } }
          : {
              type,
              file_item: {
                media,
                file_name: name || basename(path),
                len: String(plaintext.length),
              },
            }
    const clientId = `pisper-${randomUUID()}`
    const sent = await this.authPost(connection, 'ilink/bot/sendmessage', {
      msg: {
        from_user_id: '',
        to_user_id: to,
        client_id: clientId,
        message_type: 2,
        message_state: 2,
        item_list: [item],
        context_token: contextToken || undefined,
      },
    })
    if (sent.ret && sent.ret !== 0) {
      if (sent.ret === -2 && sent.errmsg === 'prepare failed')
        throw new Error('微信会话上下文已失效，请先在微信中向机器人发送一条消息后重试。')
      throw new Error(sent.errmsg || `微信媒体发送失败（${sent.ret}）`)
    }
    return { messageId: clientId }
  }
}

export function weixinTextFromItems(items = []) {
  return items
    .map((item) =>
      item.type === MESSAGE_ITEM.TEXT
        ? item.text_item?.text || ''
        : item.type === MESSAGE_ITEM.VOICE
          ? item.voice_item?.text || ''
          : '',
    )
    .filter(Boolean)
    .join('\n')
    .trim()
}

export function weixinMediaItems(items = []) {
  return items.filter((item) =>
    [MESSAGE_ITEM.IMAGE, MESSAGE_ITEM.FILE, MESSAGE_ITEM.VIDEO].includes(item.type),
  )
}
