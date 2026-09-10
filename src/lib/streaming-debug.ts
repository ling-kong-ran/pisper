// 流式调试时间线：只在开发环境记录关键阶段，帮助区分 SSE、打字机和渲染层的停顿。
type StreamingDebugEvent = {
  type: string
  at: number
  length?: number
  detail?: string
}

const MAX_EVENTS = 500

export function recordStreamingDebug(type: string, length?: number, detail?: string) {
  if (!import.meta.env?.DEV) return
  const target = globalThis as typeof globalThis & {
    __PISPER_STREAM_DEBUG__?: StreamingDebugEvent[]
  }
  const events = (target.__PISPER_STREAM_DEBUG__ ??= [])
  events.push({ type, at: performance.now(), length, detail })
  if (events.length > MAX_EVENTS) events.splice(0, events.length - MAX_EVENTS)
}

export function clearStreamingDebug() {
  const target = globalThis as typeof globalThis & {
    __PISPER_STREAM_DEBUG__?: StreamingDebugEvent[]
  }
  target.__PISPER_STREAM_DEBUG__ = []
}
