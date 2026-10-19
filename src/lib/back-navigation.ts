// 返回层只保存关闭回调；不写路由历史，不拥有任务或弹层业务状态。
export function createBackStack() {
  let next = 0
  const layers = new Map<number, { priority: number; close: () => void }>()
  return {
    register(priority: number, close: () => void) {
      const id = ++next
      layers.set(id, { priority, close })
      return () => {
        layers.delete(id)
      }
    },
    dismiss() {
      const top = [...layers.entries()].sort(
        ([a, x], [b, y]) => y.priority - x.priority || b - a,
      )[0]
      if (!top) return false
      top[1].close()
      return true
    },
  }
}
export const appBackStack = createBackStack()
