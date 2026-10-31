// 卡死警告（计划 §4.3）：React 的 <Profiler onRender> 在生产包里不会触发（React 19，没有 profiling 构建），
// 改为“渲染阶段记开始时间 + 子树末尾哨兵的 layout effect 计算本次提交耗时”。
// 连续 3 次超过 200ms 才给出警告：偶发的一次慢提交（首次加载字体、冷缓存）不该打扰用户。

export const SLOW_COMMIT_MS = 200
export const SLOW_COMMIT_STREAK = 3

export type CommitSample = { duration: number; slow: boolean; warning: boolean }

export type CommitTimer = {
  start(at: number): void
  commit(at: number): CommitSample | null
  last(): number | null
  warning(): boolean
  reset(): void
}

export function createCommitTimer({
  thresholdMs = SLOW_COMMIT_MS,
  streak = SLOW_COMMIT_STREAK,
}: { thresholdMs?: number; streak?: number } = {}): CommitTimer {
  let startedAt: number | null = null
  let consecutive = 0
  let lastDuration: number | null = null
  let warned = false
  return {
    start(at) {
      // 同一次提交里渲染可能被重放（并发渲染、StrictMode）：以最早的开始时间为准。
      if (startedAt === null) startedAt = at
    },
    commit(at) {
      if (startedAt === null) return null
      const duration = Math.max(0, at - startedAt)
      startedAt = null
      lastDuration = duration
      const slow = duration > thresholdMs
      consecutive = slow ? consecutive + 1 : 0
      if (consecutive >= streak) warned = true
      return { duration, slow, warning: warned }
    },
    last: () => lastDuration,
    warning: () => warned,
    reset() {
      startedAt = null
      consecutive = 0
      lastDuration = null
      warned = false
    },
  }
}
