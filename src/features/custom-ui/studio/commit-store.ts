// 预览渲染耗时：每个工作室页面一份，状态栏订阅。
export function createCommitStore() {
  let value: number | null = null
  const listeners = new Set<() => void>()
  return {
    set(next: number) {
      const rounded = Math.round(next * 10) / 10
      if (rounded === value) return
      value = rounded
      for (const listener of [...listeners]) listener()
    },
    get: () => value,
    subscribe(listener: () => void) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
  }
}
export type CommitStore = ReturnType<typeof createCommitStore>
