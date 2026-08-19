// 全局 UI 偏好持久化到 localStorage；迁移层保留旧版按时间自动主题的行为，
// 避免升级后用户在相同环境下看到意外的明暗变化。
import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { STORAGE_KEYS } from '@/app/storage'

export type ThemeMode = 'system' | 'scheduled' | 'light' | 'dark'
export type DensityMode = 'comfortable' | 'compact'
export type AccentPreset = 'neutral' | 'blue' | 'teal' | 'violet' | 'coral'
export type FontScale = 'small' | 'default' | 'large'
export type RadiusMode = 'sharp' | 'default' | 'soft'
export type MotionMode = 'system' | 'full' | 'reduced'

export const DEFAULT_UI_PREFERENCES = {
  theme: 'system',
  density: 'comfortable',
  accent: 'neutral',
  fontScale: 'default',
  radius: 'default',
  motion: 'system',
} as const satisfies {
  theme: ThemeMode
  density: DensityMode
  accent: AccentPreset
  fontScale: FontScale
  radius: RadiusMode
  motion: MotionMode
}

type UiState = {
  sidebarCollapsed: boolean
  theme: ThemeMode
  density: DensityMode
  accent: AccentPreset
  fontScale: FontScale
  radius: RadiusMode
  motion: MotionMode
  setSidebarCollapsed: (collapsed: boolean) => void
  toggleSidebar: () => void
  setTheme: (theme: ThemeMode) => void
  cycleTheme: () => void
  setDensity: (density: DensityMode) => void
  setAccent: (accent: AccentPreset) => void
  setFontScale: (fontScale: FontScale) => void
  setRadius: (radius: RadiusMode) => void
  setMotion: (motion: MotionMode) => void
  resetAppearance: () => void
}

type PersistedUiState = Pick<
  UiState,
  'sidebarCollapsed' | 'theme' | 'density' | 'accent' | 'fontScale' | 'radius' | 'motion'
>

const THEME_SEQUENCE: ThemeMode[] = ['system', 'scheduled', 'light', 'dark']

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      sidebarCollapsed: false,
      ...DEFAULT_UI_PREFERENCES,
      setSidebarCollapsed: (sidebarCollapsed) => set({ sidebarCollapsed }),
      toggleSidebar: () => set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed })),
      setTheme: (theme) => set({ theme }),
      cycleTheme: () =>
        set((state) => ({
          theme: THEME_SEQUENCE[(THEME_SEQUENCE.indexOf(state.theme) + 1) % THEME_SEQUENCE.length],
        })),
      setDensity: (density) => set({ density }),
      setAccent: (accent) => set({ accent }),
      setFontScale: (fontScale) => set({ fontScale }),
      setRadius: (radius) => set({ radius }),
      setMotion: (motion) => set({ motion }),
      resetAppearance: () => set(DEFAULT_UI_PREFERENCES),
    }),
    {
      name: 'pisper-ui',
      version: 1,
      migrate: (persisted, version): PersistedUiState => {
        const stored = persisted as Partial<PersistedUiState>
        return {
          sidebarCollapsed: stored.sidebarCollapsed ?? false,
          ...DEFAULT_UI_PREFERENCES,
          ...stored,
          theme:
            version === 0 && stored.theme === 'system' ? 'scheduled' : (stored.theme ?? 'system'),
        }
      },
      partialize: ({ sidebarCollapsed, theme, density, accent, fontScale, radius, motion }) => ({
        sidebarCollapsed,
        theme,
        density,
        accent,
        fontScale,
        radius,
        motion,
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
            (legacyTheme === 'system'
              ? 'scheduled'
              : THEME_SEQUENCE.includes(legacyTheme as ThemeMode)
                ? (legacyTheme as ThemeMode)
                : current.theme),
          sidebarCollapsed: stored?.sidebarCollapsed ?? legacySidebar === '1',
          density: stored?.density ?? (legacyDensity === 'compact' ? 'compact' : current.density),
          accent: stored?.accent ?? current.accent,
          fontScale: stored?.fontScale ?? current.fontScale,
          radius: stored?.radius ?? current.radius,
          motion: stored?.motion ?? current.motion,
        }
      },
    },
  ),
)
