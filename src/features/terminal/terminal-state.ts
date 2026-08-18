// 终端面板状态持久化：开关与高度存入 localStorage，跨重启恢复。
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
