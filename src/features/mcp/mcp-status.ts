// @public “更多”启动台的 MCP 磁贴角标：在线服务数。用 refresh=0 只读 Runtime 已有的连接快照，
// 不为一个角标去逐个探测服务；响应按不可信数据校验。
import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'

export function parseOnlineMcpCount(value: unknown): number {
  const metrics =
    value && typeof value === 'object' ? (value as { metrics?: unknown }).metrics : null
  const online =
    metrics && typeof metrics === 'object'
      ? (metrics as { onlineServices?: unknown }).onlineServices
      : undefined
  if (typeof online !== 'number' || !Number.isFinite(online) || online < 0)
    throw invalidResponseError()
  return Math.floor(online)
}

export function fetchOnlineMcpCount(signal?: AbortSignal) {
  return requestJson('/api/mcp?refresh=0', { signal, parse: parseOnlineMcpCount })
}
