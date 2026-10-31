// 能力台账的浏览器侧上报（A8，规格 5.4）：notify 与 sessions.* 只发生在浏览器里，按组件攒成批，
// 5 秒一次 POST …/usage（keepalive，关页前的最后一批也尽量送达）。失败只丢这一批：台账是“看得见”的辅助，
// 不重试、不影响组件本身。网络与存储由 Runtime 在代理 / 存储路由里自己记，不经过这里。
export type BrowserCapability = 'notify' | 'sessions.read' | 'sessions.open'
export type UsageEntry = { capability: BrowserCapability; count: number; at: string }
export type UsageSender = (componentId: string, entries: UsageEntry[]) => Promise<unknown>

export const USAGE_FLUSH_MS = 5000

export function createUsageReporter({
  send,
  now = () => Date.now(),
  schedule = (callback, ms) => window.setTimeout(callback, ms),
}: {
  send: UsageSender
  now?: () => number
  schedule?: (callback: () => void, ms: number) => unknown
}) {
  // componentId → capability → { 次数, 最后一次的时间 }
  let pending = new Map<string, Map<BrowserCapability, { count: number; at: number }>>()
  let scheduled = false
  const flush = () => {
    scheduled = false
    const batch = pending
    pending = new Map()
    for (const [componentId, entries] of batch) {
      const list = [...entries].map(([capability, entry]) => ({
        capability,
        count: entry.count,
        at: new Date(entry.at).toISOString(),
      }))
      void send(componentId, list).catch(() => undefined)
    }
  }
  return {
    record(componentId: string, capability: BrowserCapability) {
      const entries = pending.get(componentId) ?? new Map()
      const current = entries.get(capability)
      entries.set(capability, { count: (current?.count ?? 0) + 1, at: now() })
      pending.set(componentId, entries)
      if (scheduled) return
      scheduled = true
      schedule(flush, USAGE_FLUSH_MS)
    },
    flush,
  }
}
