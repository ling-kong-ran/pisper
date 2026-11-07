// @public 已接入的聊天渠道，以及跟 Pisper 说过话的群聊：Bot 设置里“可以叫它的聊天”据此直接点选，
// 不必去聊天里发 /whoami 查 ID。响应按不可信数据校验。
import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'

export type ChatPlatform = 'feishu' | 'weixin' | 'qq' | 'telegram'

const PLATFORMS: readonly ChatPlatform[] = ['feishu', 'weixin', 'qq', 'telegram']

export type ConnectedChannel = {
  platform: ChatPlatform
  /** 接入时记下了创建者身份：没写名单的规则靠它认人。 */
  ownerConfigured: boolean
}

export type KnownGroupChat = {
  platform: ChatPlatform
  peerId: string
  title: string
  lastMessage: string
}

export type ChannelChats = { channels: ConnectedChannel[]; groups: KnownGroupChat[] }

const isPlatform = (value: unknown): value is ChatPlatform =>
  PLATFORMS.includes(value as ChatPlatform)

export function parseChannelChats(value: unknown): ChannelChats {
  const input = value && typeof value === 'object' ? (value as Record<string, unknown>) : null
  const connections = input?.connections
  if (!connections || typeof connections !== 'object' || Array.isArray(connections))
    throw invalidResponseError()
  const channels = PLATFORMS.flatMap((platform) => {
    const connection = (connections as Record<string, unknown>)[platform]
    if (!connection || typeof connection !== 'object') return []
    const { enabled, ownerConfigured } = connection as Record<string, unknown>
    return enabled === false ? [] : [{ platform, ownerConfigured: ownerConfigured === true }]
  })
  const connected = new Set(channels.map((channel) => channel.platform))
  const scopes = Array.isArray(input.scopes) ? input.scopes : []
  const groups = scopes.flatMap((scope) => {
    const item = scope && typeof scope === 'object' ? (scope as Record<string, unknown>) : {}
    const text = (field: unknown) => (typeof field === 'string' ? field.trim() : '')
    const peerId = text(item.peerId)
    if (item.chatType !== 'group' || !isPlatform(item.platform) || !peerId) return []
    if (!connected.has(item.platform)) return []
    return [
      {
        platform: item.platform,
        peerId,
        title: text(item.title) || peerId,
        lastMessage: text(item.lastMessage),
      },
    ]
  })
  return { channels, groups }
}

export function fetchChannelChats(signal?: AbortSignal) {
  return requestJson('/api/channels', { signal, parse: parseChannelChats })
}
