// 页面主操作注册表：页面把自己标题栏的主按钮（如“新建会话”）注册进来，
// 壳层（Cmd+K / 空状态）可统一触发。invoke 时若页面尚未注册（首帧），
// 先记录 queued，待注册时立即补触发，避免丢操作。
export type PrimaryAction = () => void

export type PrimaryActionRegistry = {
  register: (action: PrimaryAction) => () => void
  invoke: () => void
  clear: () => void
}

// 主操作注册表：register 覆盖当前操作并返回注销函数；
// invoke 在无操作时置 queued 标记，等 register 到来时补触发一次，
// 解决启动早期（页面尚未挂载）触发主操作被丢失的问题。
export function createPrimaryActionRegistry(): PrimaryActionRegistry {
  let currentAction: PrimaryAction | null = null
  let queued = false

  return {
    register(action) {
      currentAction = action
      if (queued) {
        queued = false
        action()
      }
      return () => {
        if (currentAction === action) currentAction = null
      }
    },
    invoke() {
      if (currentAction) return currentAction()
      queued = true
      return undefined
    },
    clear() {
      currentAction = null
    },
  }
}
