// 熔断（规格 5.6、计划 §4.3）：同一实例 60 秒内崩溃 3 次就暂停，显示“已暂停”卡，手动恢复。
// 状态只在页面内存里：刷新页面即恢复，避免一次坏版本把实例永久锁死；实例重挂（热重载、路由切换）不清零。

export const CIRCUIT_WINDOW_MS = 60_000
export const CIRCUIT_THRESHOLD = 3

export type CircuitState = { crashes: number[]; paused: boolean }

/** 纯函数：记一次崩溃，返回新状态（窗口外的旧记录丢弃）。 */
export function recordCrash(
  state: CircuitState | undefined,
  at: number,
  { windowMs = CIRCUIT_WINDOW_MS, threshold = CIRCUIT_THRESHOLD } = {},
): CircuitState {
  const crashes = [...(state?.crashes ?? []).filter((time) => at - time < windowMs), at]
  return { crashes, paused: state?.paused === true || crashes.length >= threshold }
}

export type CircuitBreaker = {
  record(key: string): CircuitState
  pause(key: string): void
  resume(key: string): void
  isPaused(key: string): boolean
  // 本页里崩溃过（还在记录里）或被暂停：工作室“需处理”筛选用。
  hasTrouble(key: string): boolean
  subscribe(listener: () => void): () => void
  // useSyncExternalStore 的快照：任一实例状态变化都会换一个新数字。
  version(): number
}

export function createCircuitBreaker({
  now = () => Date.now(),
  windowMs = CIRCUIT_WINDOW_MS,
  threshold = CIRCUIT_THRESHOLD,
}: { now?: () => number; windowMs?: number; threshold?: number } = {}): CircuitBreaker {
  const states = new Map<string, CircuitState>()
  const listeners = new Set<() => void>()
  let revision = 0
  const changed = () => {
    revision += 1
    for (const listener of [...listeners]) listener()
  }
  return {
    record(key) {
      const next = recordCrash(states.get(key), now(), { windowMs, threshold })
      states.set(key, next)
      changed()
      return next
    },
    pause(key) {
      states.set(key, { crashes: states.get(key)?.crashes ?? [], paused: true })
      changed()
    },
    resume(key) {
      // 恢复时清空崩溃记录：否则恢复后的第一次崩溃就会立刻再次暂停。
      states.delete(key)
      changed()
    },
    isPaused: (key) => states.get(key)?.paused === true,
    hasTrouble: (key) => {
      const state = states.get(key)
      return Boolean(state && (state.paused || state.crashes.length > 0))
    },
    subscribe(listener) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    version: () => revision,
  }
}
