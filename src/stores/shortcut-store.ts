import { create } from 'zustand'
import { STORAGE_KEYS } from '@/app/storage'
import {
  DEFAULT_SHORTCUTS,
  normalizeShortcutBindings,
  validateShortcutBindings,
  type ShortcutAction,
  type ShortcutBindings,
} from '@shared/shortcuts.mjs'

function readBindings(): ShortcutBindings {
  try {
    return normalizeShortcutBindings(
      JSON.parse(localStorage.getItem(STORAGE_KEYS.shortcuts) || 'null'),
    )
  } catch {
    return { ...DEFAULT_SHORTCUTS }
  }
}

type ShortcutState = {
  bindings: ShortcutBindings
  setBinding: (action: ShortcutAction, binding: string | null) => void
  resetBindings: () => void
}

export const useShortcutStore = create<ShortcutState>((set, get) => {
  const save = (bindings: ShortcutBindings) => {
    // 先确认持久化成功，再让界面和实际按键一起生效，避免保存失败却显示已保存。
    localStorage.setItem(STORAGE_KEYS.shortcuts, JSON.stringify(bindings))
    set({ bindings })
  }
  return {
    bindings: readBindings(),
    setBinding: (action, binding) =>
      save(validateShortcutBindings({ ...get().bindings, [action]: binding })),
    resetBindings: () => save({ ...DEFAULT_SHORTCUTS }),
  }
})

if (typeof window !== 'undefined') {
  window.addEventListener('storage', (event) => {
    if (event.key === STORAGE_KEYS.shortcuts || event.key === null) {
      useShortcutStore.setState({ bindings: readBindings() })
    }
  })
}
