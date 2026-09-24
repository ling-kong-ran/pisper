// 上下文宽度属于聊天布局偏好；会话切换和外观重置不应改变用户选择。
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import {
  normalizeSessionContextWidth,
  SESSION_CONTEXT_DEFAULT_WIDTH,
} from '@/features/chat/session-context-layout'

type SessionContextPreferences = {
  width: number
}

type SessionContextState = SessionContextPreferences & {
  setWidth: (value: number) => void
}

// 隐私模式、存储配额或 WebView 策略可能禁用本地存储；仍保留本次页面中的调整。
const storage = createJSONStorage<SessionContextPreferences>(() => ({
  getItem: (name) => {
    try {
      return window.localStorage.getItem(name)
    } catch {
      return null
    }
  },
  setItem: (name, value) => {
    try {
      window.localStorage.setItem(name, value)
    } catch {
      // 持久化失败不回滚当前布局，也不打断鼠标和键盘交互。
    }
  },
  removeItem: (name) => {
    try {
      window.localStorage.removeItem(name)
    } catch {
      // 存储不可用时没有可清理的持久化状态。
    }
  },
}))

export const useSessionContextStore = create<SessionContextState>()(
  persist(
    (set, get) => ({
      width: SESSION_CONTEXT_DEFAULT_WIDTH,
      setWidth: (value) => {
        const width = normalizeSessionContextWidth(value)
        if (get().width !== width) set({ width })
      },
    }),
    {
      name: 'pisper-session-context-layout',
      version: 1,
      storage,
      partialize: ({ width }) => ({ width }),
      merge: (persisted, current) => ({
        ...current,
        width: normalizeSessionContextWidth(
          persisted &&
            typeof persisted === 'object' &&
            !Array.isArray(persisted) &&
            'width' in persisted
            ? persisted.width
            : undefined,
        ),
      }),
    },
  ),
)
