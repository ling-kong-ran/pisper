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

// 当前是否有打开着的弹层（对话框、菜单、弹出层）：Esc 先交给它们，设置的“← 返回 Esc”让路。
// 判定与 BackNavigation 的 hasOverlay 相同（那一份由 back-navigation 守卫按源码在隔离上下文里执行，暂不改动）。
export function hasOpenOverlay() {
  return [
    ...document.querySelectorAll<HTMLElement>(
      '[role="dialog"], [role="alertdialog"], [role="menu"], [data-slot="popover-content"]',
    ),
  ].some(
    (element) =>
      element.dataset.state !== 'closed' &&
      element.getAttribute('aria-hidden') !== 'true' &&
      element.getClientRects().length > 0,
  )
}
