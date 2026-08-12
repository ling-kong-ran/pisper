import { STORAGE_KEYS } from '@/app/storage'

type StoredPanelState = {
  open?: boolean
  height?: number
}

export function readStoredTerminalPanel(): StoredPanelState {
  try {
    return JSON.parse(localStorage.getItem(STORAGE_KEYS.terminalPanel) || '{}')
  } catch {
    return {}
  }
}
