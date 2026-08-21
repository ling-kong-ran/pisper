// 读取桌面壳发布的 Iroh 隧道状态。Node Runtime 不持有 Iroh socket，
// 仅把经过校验的连接元数据加入现有配对协议。
import { readFileSync } from 'node:fs'

function normalizeDirectAddresses(value) {
  if (!Array.isArray(value)) return []
  return value
    .map((item) => String(item || '').trim())
    .filter((item) => /^\[[0-9a-f:]+\]:\d+$/i.test(item) || /^[^\s:]+:\d+$/.test(item))
    .slice(0, 32)
}

export function readIrohTunnelStatus(path = process.env.PISPER_IROH_STATUS_FILE) {
  const unavailable = {
    available: false,
    relayConnected: false,
    nodeId: null,
    endpoint: null,
    error: null,
  }
  if (!path) return unavailable
  try {
    const parsed = JSON.parse(readFileSync(path, 'utf8'))
    const endpoint = parsed?.version === 1 ? parsed.endpoint : null
    const nodeId = String(endpoint?.nodeId || '').trim()
    if (!nodeId) {
      return {
        ...unavailable,
        error: typeof parsed?.error === 'string' ? parsed.error : null,
      }
    }
    const relayUrl = String(endpoint?.relayUrl || '').trim() || null
    const directAddresses = normalizeDirectAddresses(endpoint?.directAddresses)
    return {
      available: Boolean(relayUrl || directAddresses.length),
      relayConnected: Boolean(relayUrl),
      nodeId,
      endpoint: {
        t: 'iroh',
        nodeId,
        ...(relayUrl ? { relayUrl } : {}),
        directAddresses,
      },
      error: typeof parsed?.error === 'string' ? parsed.error : null,
    }
  } catch (error) {
    if (error?.code === 'ENOENT') return unavailable
    return {
      ...unavailable,
      error: error instanceof Error ? error.message : String(error),
    }
  }
}
