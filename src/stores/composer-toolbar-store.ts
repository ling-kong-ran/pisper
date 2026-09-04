// 输入框快捷栏偏好独立持久化，避免恢复主题等外观设置时连带重置工具位置。
import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import {
  DEFAULT_COMPOSER_TOOLBAR_LAYOUT,
  moveComposerTool,
  normalizeComposerToolbarLayout,
  setComposerToolLocation,
  type ComposerToolbarLayout,
  type ComposerToolId,
  type ComposerToolLocation,
} from '@/features/chat/composer-toolbar-layout'

type ComposerToolbarState = {
  layout: ComposerToolbarLayout
  setToolLocation: (id: ComposerToolId, location: ComposerToolLocation) => void
  moveTool: (id: ComposerToolId, direction: -1 | 1) => void
  resetLayout: () => void
}

export const useComposerToolbarStore = create<ComposerToolbarState>()(
  persist(
    (set) => ({
      layout: DEFAULT_COMPOSER_TOOLBAR_LAYOUT,
      setToolLocation: (id, location) =>
        set((state) => ({ layout: setComposerToolLocation(state.layout, id, location) })),
      moveTool: (id, direction) =>
        set((state) => ({ layout: moveComposerTool(state.layout, id, direction) })),
      resetLayout: () => set({ layout: DEFAULT_COMPOSER_TOOLBAR_LAYOUT }),
    }),
    {
      name: 'pisper-composer-toolbar',
      version: 1,
      partialize: ({ layout }) => ({ layout }),
      merge: (persisted, current) => ({
        ...current,
        layout: normalizeComposerToolbarLayout(
          (persisted as Partial<ComposerToolbarState> | undefined)?.layout,
        ),
      }),
    },
  ),
)
