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
