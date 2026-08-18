// 页面主操作注册表：页面把自己标题栏的主按钮（如“新建会话”）注册进来，
// 壳层（Cmd+K / 空状态）可统一触发。invoke 时若页面尚未注册（首帧），
// 先记录 queued，待注册时立即补触发，避免丢操作。
export type PrimaryAction = () => void

export type PrimaryActionRegistry = {
  register: (action: PrimaryAction) => () => void
  invoke: () => void
  clear: () => void
}

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
