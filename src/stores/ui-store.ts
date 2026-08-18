// 全局 UI 偏好（侧边栏折叠 / 主题 / 密度），持久化到 localStorage。
// merge 阶段兼容迁移旧版本遗留的独立 localStorage key（如 pisper-theme），
// 保证升级后用户既有设置不丢失；主题循环顺序 system → light → dark。
import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { STORAGE_KEYS } from '@/app/storage'

export type ThemeMode = 'system' | 'light' | 'dark'
export type DensityMode = 'comfortable' | 'compact'

type UiState = {
  sidebarCollapsed: boolean
  theme: ThemeMode
  density: DensityMode
  setSidebarCollapsed: (collapsed: boolean) => void
  toggleSidebar: () => void
  setTheme: (theme: ThemeMode) => void
  cycleTheme: () => void
  setDensity: (density: DensityMode) => void
}

const THEME_SEQUENCE: ThemeMode[] = ['system', 'light', 'dark']

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      sidebarCollapsed: false,
      theme: 'system',
      density: 'comfortable',
      setSidebarCollapsed: (sidebarCollapsed) => set({ sidebarCollapsed }),
      toggleSidebar: () => set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed })),
      setTheme: (theme) => set({ theme }),
      cycleTheme: () =>
        set((state) => ({
          theme: THEME_SEQUENCE[(THEME_SEQUENCE.indexOf(state.theme) + 1) % THEME_SEQUENCE.length],
        })),
      setDensity: (density) => set({ density }),
    }),
    {
      name: 'pisper-ui',
      partialize: ({ sidebarCollapsed, theme, density }) => ({
        sidebarCollapsed,
        theme,
        density,
      }),
      merge: (persisted, current) => {
        const stored = persisted as Partial<UiState> | undefined
        const legacyTheme = localStorage.getItem(STORAGE_KEYS.theme)
        const legacySidebar = localStorage.getItem(STORAGE_KEYS.sidebarCollapsed)
        const legacyDensity = localStorage.getItem(STORAGE_KEYS.density)
        return {
          ...current,
          ...stored,
          theme:
            stored?.theme ??
            (THEME_SEQUENCE.includes(legacyTheme as ThemeMode)
              ? (legacyTheme as ThemeMode)
              : current.theme),
          sidebarCollapsed: stored?.sidebarCollapsed ?? legacySidebar === '1',
          density: stored?.density ?? (legacyDensity === 'compact' ? 'compact' : current.density),
        }
      },
    },
  ),
)
