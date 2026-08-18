// 终端面板状态持久化：开关与高度存入 localStorage，跨重启恢复。
import { STORAGE_KEYS } from '@/app/storage'

type StoredPanelState = {
  open?: boolean
  height?: number
}

// 读取持久化的终端面板状态（开关/高度），非法值回退默认。
export function readStoredTerminalPanel(): StoredPanelState {
  try {
    return JSON.parse(localStorage.getItem(STORAGE_KEYS.terminalPanel) || '{}')
  } catch {
    return {}
  }
}
