// 托管定时器（规格 5.6）：组件经 ctx.setTimeout / setInterval 建的定时器在卸载或热重载时自动清理；
// 回调异常归属到这个实例（记日志、计入熔断），不会变成未处理的全局错误。
// 只清理自己建的 id：组件误传别人的 id 时不动它。

export type TimerHost = {
  setTimeout(handler: () => void, ms: number): number
  setInterval(handler: () => void, ms: number): number
  clearTimeout(id: number): void
  clearInterval(id: number): void
}

export type ManagedTimers = {
  setTimeout(handler: unknown, ms?: unknown): number
  setInterval(handler: unknown, ms?: unknown): number
  clearTimeout(id: unknown): void
  clearInterval(id: unknown): void
  active(): number
  dispose(): void
}

// 间隔下限：防止 setInterval(fn, 0) 把页面拖进忙循环。
export const MIN_INTERVAL_MS = 16
const MAX_DELAY_MS = 2_147_483_647

function delay(ms: unknown, minimum: number) {
  const value = typeof ms === 'number' && Number.isFinite(ms) ? ms : 0
  return Math.min(MAX_DELAY_MS, Math.max(minimum, value))
}

export function createManagedTimers({
  host,
  onError,
}: {
  host: TimerHost
  onError: (error: unknown) => void
}): ManagedTimers {
  const timeouts = new Set<number>()
  const intervals = new Set<number>()
  let disposed = false
  const guard = (handler: () => void) => {
    try {
      handler()
    } catch (error) {
      onError(error)
    }
  }
  return {
    setTimeout(handler, ms) {
      if (disposed || typeof handler !== 'function') return 0
      const id = host.setTimeout(
        () => {
          timeouts.delete(id)
          if (!disposed) guard(handler as () => void)
        },
        delay(ms, 0),
      )
      timeouts.add(id)
      return id
    },
    setInterval(handler, ms) {
      if (disposed || typeof handler !== 'function') return 0
      const id = host.setInterval(
        () => {
          if (!disposed) guard(handler as () => void)
        },
        delay(ms, MIN_INTERVAL_MS),
      )
      intervals.add(id)
      return id
    },
    clearTimeout(id) {
      if (typeof id !== 'number' || !timeouts.delete(id)) return
      host.clearTimeout(id)
    },
    clearInterval(id) {
      if (typeof id !== 'number' || !intervals.delete(id)) return
      host.clearInterval(id)
    },
    active: () => timeouts.size + intervals.size,
    dispose() {
      disposed = true
      for (const id of timeouts) host.clearTimeout(id)
      for (const id of intervals) host.clearInterval(id)
      timeouts.clear()
      intervals.clear()
    },
  }
}
