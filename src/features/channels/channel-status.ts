// @public “更多”启动台的渠道磁贴角标：已启用但没有连上的渠道数（需要人处理，琥珀色）。
// 正在连接 / 重连中的不算断开，避免启动瞬间误报；响应按不可信数据校验。
import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'

const LIVE_STATES = new Set(['connected', 'connecting', 'reconnecting'])

export function parseDisconnectedChannelCount(value: unknown): number {
  const connections =
    value && typeof value === 'object' ? (value as { connections?: unknown }).connections : null
  if (!connections || typeof connections !== 'object' || Array.isArray(connections))
    throw invalidResponseError()
  let count = 0
  for (const connection of Object.values(connections)) {
    if (!connection || typeof connection !== 'object') continue
    const { enabled, status } = connection as { enabled?: unknown; status?: unknown }
    if (enabled === false) continue
    if (typeof status !== 'string' || !LIVE_STATES.has(status)) count += 1
  }
  return count
}

export function fetchDisconnectedChannelCount(signal?: AbortSignal) {
  return requestJson('/api/channels', { signal, parse: parseDisconnectedChannelCount })
}
