// 桌面前端和 Runtime 之间的指令通道（runtime/services/desktop-host-channel.mjs）：
// 用 SSE 收指令，用 POST 回结果。断了就退避重连：Runtime 可能在重启或还没就绪。
import { consumeEventStream } from '@/lib/event-stream'

const MIN_RETRY_MS = 1_000
const MAX_RETRY_MS = 30_000

export async function connectDesktopHost(
  endpoint: string,
  signal: AbortSignal,
  onEvent: (event: string, data: unknown) => void,
  onDisconnect?: () => void,
) {
  let delay = MIN_RETRY_MS
  while (!signal.aborted) {
    try {
      const response = await fetch(endpoint, { signal })
      await consumeEventStream<unknown>(response, (event, data) => {
        delay = MIN_RETRY_MS
        onEvent(event, data)
      })
    } catch {
      // 下面统一退避。
    }
    onDisconnect?.()
    if (signal.aborted) return
    await new Promise((resolve) => setTimeout(resolve, delay))
    delay = Math.min(MAX_RETRY_MS, delay * 2)
  }
}

export function reportDesktopHostResult(
  endpoint: string,
  requestId: string,
  payload: Record<string, unknown>,
) {
  return fetch(`${endpoint}/${encodeURIComponent(requestId)}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  }).catch(() => {})
}
