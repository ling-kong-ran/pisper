import { create } from 'zustand'
import { STORAGE_KEYS } from '@/app/storage'
import { pageStateStorage } from '@/lib/page-state-storage'
import {
  DEFAULT_BOARD_SHORTCUTS,
  DEFAULT_SHORTCUTS,
  normalizeBoardShortcutBindings,
  normalizeShortcutBindings,
  validateBoardShortcutBindings,
  validateShortcutBindings,
  type BoardShortcutAction,
  type BoardShortcutBindings,
  type ShortcutAction,
  type ShortcutBindings,
} from '@shared/shortcuts.mjs'

function readBindings(): ShortcutBindings {
  try {
    return normalizeShortcutBindings(
      JSON.parse(pageStateStorage.getItem(STORAGE_KEYS.shortcuts) || 'null'),
    )
  } catch {
    return { ...DEFAULT_SHORTCUTS }
  }
}

function readBoardBindings(): BoardShortcutBindings {
  try {
    return normalizeBoardShortcutBindings(
      JSON.parse(pageStateStorage.getItem(STORAGE_KEYS.boardShortcuts) || 'null'),
    )
  } catch {
    return { ...DEFAULT_BOARD_SHORTCUTS }
  }
}

type ShortcutState = {
  bindings: ShortcutBindings
  // 看板单字母键单独成表：只在看板页、焦点不在输入框时生效，校验规则与应用级组合键不同。
  boardBindings: BoardShortcutBindings
  setBinding: (action: ShortcutAction, binding: string | null) => void
  setBoardBinding: (action: BoardShortcutAction, binding: string | null) => void
  resetBindings: () => void
}

export const useShortcutStore = create<ShortcutState>((set, get) => {
  const save = (bindings: ShortcutBindings) => {
    // 先确认持久化成功，再让界面和实际按键一起生效，避免保存失败却显示已保存。
    pageStateStorage.setItem(STORAGE_KEYS.shortcuts, JSON.stringify(bindings))
    set({ bindings })
  }
  const saveBoard = (boardBindings: BoardShortcutBindings) => {
    pageStateStorage.setItem(STORAGE_KEYS.boardShortcuts, JSON.stringify(boardBindings))
    set({ boardBindings })
  }
  return {
    bindings: readBindings(),
    boardBindings: readBoardBindings(),
    setBinding: (action, binding) =>
      save(validateShortcutBindings({ ...get().bindings, [action]: binding })),
    setBoardBinding: (action, binding) =>
      saveBoard(validateBoardShortcutBindings({ ...get().boardBindings, [action]: binding })),
    resetBindings: () => {
      save({ ...DEFAULT_SHORTCUTS })
      saveBoard({ ...DEFAULT_BOARD_SHORTCUTS })
    },
  }
})

if (typeof window !== 'undefined') {
  window.addEventListener('storage', (event) => {
    if (event.key === STORAGE_KEYS.shortcuts || event.key === null) {
      useShortcutStore.setState({ bindings: readBindings() })
    }
    if (event.key === STORAGE_KEYS.boardShortcuts || event.key === null) {
      useShortcutStore.setState({ boardBindings: readBoardBindings() })
    }
  })
}
