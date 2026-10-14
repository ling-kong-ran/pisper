// 上下文宽度属于聊天布局偏好；会话切换和外观重置不应改变用户选择。
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import { pageStateStorage } from '@/lib/page-state-storage'
import {
  normalizeSessionContextWidth,
  SESSION_CONTEXT_DEFAULT_WIDTH,
} from '@/features/chat/session-context-layout'

type SessionContextPreferences = {
  width: number
  open: boolean
}

type SessionContextState = SessionContextPreferences & {
  setWidth: (value: number) => void
  setOpen: (value: boolean) => void
}

// 隐私模式、存储配额或 WebView 策略可能禁用本地存储；仍保留本次页面中的调整。
const storage = createJSONStorage<SessionContextPreferences>(() => ({
  getItem: (name) => {
    try {
      return pageStateStorage.getItem(name)
    } catch {
      return null
    }
  },
  setItem: (name, value) => {
    try {
      pageStateStorage.setItem(name, value)
    } catch {
      // 持久化失败不回滚当前布局，也不打断鼠标和键盘交互。
    }
  },
  removeItem: (name) => {
    try {
      pageStateStorage.removeItem(name)
    } catch {
      // 存储不可用时没有可清理的持久化状态。
    }
  },
}))

export const useSessionContextStore = create<SessionContextState>()(
  persist(
    (set, get) => ({
      width: SESSION_CONTEXT_DEFAULT_WIDTH,
      open: false,
      setWidth: (value) => {
        const width = normalizeSessionContextWidth(value)
        if (get().width !== width) set({ width })
      },
      setOpen: (open) => {
        if (get().open !== open) set({ open })
      },
    }),
    {
      name: 'pisper-session-context-layout',
      version: 2,
      storage,
      migrate: (persisted) => {
        const previous =
          persisted && typeof persisted === 'object' && !Array.isArray(persisted) ? persisted : {}
        return {
          width: normalizeSessionContextWidth('width' in previous ? previous.width : undefined),
          open: 'open' in previous && previous.open === true,
        }
      },
      partialize: ({ width, open }) => ({ width, open }),
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
        open: Boolean(
          persisted &&
          typeof persisted === 'object' &&
          !Array.isArray(persisted) &&
          'open' in persisted &&
          persisted.open === true,
        ),
      }),
    },
  ),
)
