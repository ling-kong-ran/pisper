import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import {
  restoreFloatingPlacements,
  saveFloatingPlacement,
  type FloatingPlacement,
  type SavedFloatingPlacement,
} from './floating-placement'

type FloatingPlacementPreferences = { positions: SavedFloatingPlacement[] }
type FloatingPlacementState = FloatingPlacementPreferences & {
  setPosition: (key: string, position: FloatingPlacement) => void
  resetPosition: (key: string) => void
}

// 拖动位置是本设备偏好；存储不可用时继续使用内存，不影响计时和拖动。
const storage = createJSONStorage<FloatingPlacementPreferences>(() => ({
  getItem: (key) => {
    try {
      return window.localStorage.getItem(key)
    } catch {
      return null
    }
  },
  setItem: (key, value) => {
    try {
      window.localStorage.setItem(key, value)
    } catch {
      // 隐私模式或配额不足时保留本次页面的位置。
    }
  },
  removeItem: (key) => {
    try {
      window.localStorage.removeItem(key)
    } catch {
      // 清理失败不阻断界面复位。
    }
  },
}))

export const useFloatingPlacementStore = create<FloatingPlacementState>()(
  persist(
    (set) => ({
      positions: [],
      setPosition: (key, position) =>
        set((state) => ({ positions: saveFloatingPlacement(state.positions, key, position) })),
      resetPosition: (key) =>
        set((state) => ({ positions: state.positions.filter((entry) => entry.key !== key) })),
    }),
    {
      name: 'pisper-floating-placement',
      version: 1,
      storage,
      partialize: ({ positions }) => ({ positions }),
      merge: (persisted, current) => ({
        ...current,
        positions: restoreFloatingPlacements(
          persisted && typeof persisted === 'object' && 'positions' in persisted
            ? persisted.positions
            : undefined,
        ),
      }),
    },
  ),
)
