// 输入框快捷栏偏好独立持久化，避免恢复主题等外观设置时连带重置工具位置。
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import { pageStateStorage } from '@/lib/storage/page-state-storage'
import {
  moveComposerTool,
  normalizeComposerToolbarLayout,
  setAllComposerToolsLocation,
  setComposerToolLocation,
  type ComposerToolbarLayout,
  type ComposerToolId,
  type ComposerToolLocation,
} from '@/features/chat/model/composer-toolbar-layout'

type ComposerToolbarState = {
  layout: ComposerToolbarLayout
  setToolLocation: (id: ComposerToolId, location: ComposerToolLocation) => void
  setAllToolsLocation: (location: ComposerToolLocation) => void
  moveTool: (id: ComposerToolId, direction: -1 | 1) => void
  resetLayout: () => void
}

const LEGACY_TOOLBAR_KEY = 'pisper-zcode-composer-toolbar'
const toolbarStorage = createJSONStorage<Pick<ComposerToolbarState, 'layout'>>(() => ({
  getItem: (key) => {
    const current = pageStateStorage.getItem(key)
    if (current !== null) return current
    const legacy = pageStateStorage.getItem(LEGACY_TOOLBAR_KEY)
    if (legacy === null) return null
    try {
      JSON.parse(legacy)
      pageStateStorage.setItem(key, legacy)
      pageStateStorage.removeItem(LEGACY_TOOLBAR_KEY)
      return legacy
    } catch {
      return null
    }
  },
  setItem: (key, value) => pageStateStorage.setItem(key, value),
  removeItem: (key) => pageStateStorage.removeItem(key),
}))

export const useComposerToolbarStore = create<ComposerToolbarState>()(
  persist(
    (set) => ({
      layout: normalizeComposerToolbarLayout(undefined),
      setToolLocation: (id, location) =>
        set((state) => ({ layout: setComposerToolLocation(state.layout, id, location) })),
      setAllToolsLocation: (location) =>
        set((state) => ({ layout: setAllComposerToolsLocation(state.layout, location) })),
      moveTool: (id, direction) =>
        set((state) => ({ layout: moveComposerTool(state.layout, id, direction) })),
      resetLayout: () => set({ layout: normalizeComposerToolbarLayout(undefined) }),
    }),
    {
      name: 'pisper-composer-toolbar',
      storage: toolbarStorage,
      // 存储形状保持兼容；新增和恢复的工具统一由 merge 归一，不重置用户排序。
      version: 1,
      partialize: ({ layout }) => ({ layout }),
      merge: (persisted, current) => ({
        ...current,
        layout: normalizeComposerToolbarLayout(
          persisted && typeof persisted === 'object' && 'layout' in persisted
            ? persisted.layout
            : undefined,
        ),
      }),
    },
  ),
)
