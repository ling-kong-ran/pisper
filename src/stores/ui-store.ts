// 全局 UI 偏好通过 pageStateStorage 持久化；迁移层保留旧版按时间自动主题的行为，
// 避免升级后用户在相同环境下看到意外的明暗变化。
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import { STORAGE_KEYS } from '@/app/storage'
import { pageStateStorage } from '@/lib/page-state-storage'
import { clampSidebarWidth, DEFAULT_SIDEBAR_WIDTH } from '@/lib/sidebar-width'
import {
  CODE_FONT_SIZE,
  LEGACY_FONT_SIZES,
  legacyFontScale,
  normalizeTypographyPreferences,
  UI_FONT_SIZE,
} from '@/lib/typography-preferences'

export type ThemeMode = 'system' | 'scheduled' | 'light' | 'dark'
export type DensityMode = 'comfortable' | 'compact'
export type AccentPreset = 'neutral' | 'blue' | 'teal' | 'violet' | 'coral' | 'custom'
export type FontScale = 'small' | 'default' | 'large'
export type RadiusMode = 'sharp' | 'default' | 'soft'
export type MotionMode = 'system' | 'full' | 'reduced'

// Quiet Precision 起默认强调色为信号钴蓝（Pisper 蓝）；中性档保留为单色墨黑版本。
export const DEFAULT_UI_PREFERENCES = {
  theme: 'system',
  density: 'comfortable',
  accent: 'blue',
  customAccent: '#3d4bef',
  fontScale: 'default',
  uiFontSize: UI_FONT_SIZE.default,
  codeFontSize: CODE_FONT_SIZE.default,
  radius: 'default',
  motion: 'system',
} as const satisfies {
  theme: ThemeMode
  density: DensityMode
  accent: AccentPreset
  customAccent: string
  fontScale: FontScale
  uiFontSize: number
  codeFontSize: number
  radius: RadiusMode
  motion: MotionMode
}

// 强调色默认值修订号：旧版本无法区分「默认中性档」与「主动选择中性档」，
// 低于当前修订号的中性档一次性迁移为新默认；迁移后再次选择中性档会被保留。
// 持久化版本号由既有迁移测试固定为 2，因此用独立标记完成这次一次性迁移。
export const ACCENT_REVISION = 1

export function migrateStoredAccent(
  accent: AccentPreset | undefined,
  revision: unknown,
): AccentPreset | undefined {
  if (accent === 'neutral' && !(Number(revision) >= ACCENT_REVISION)) return 'blue'
  return accent
}

type UiState = {
  sidebarCollapsed: boolean
  sidebarWidth: number
  theme: ThemeMode
  density: DensityMode
  accent: AccentPreset
  accentRevision: number
  customAccent: string
  fontScale: FontScale
  uiFontSize: number
  codeFontSize: number
  radius: RadiusMode
  motion: MotionMode
  setSidebarCollapsed: (collapsed: boolean) => void
  setSidebarWidth: (width: number) => void
  toggleSidebar: () => void
  setTheme: (theme: ThemeMode) => void
  cycleTheme: () => void
  setDensity: (density: DensityMode) => void
  setAccent: (accent: AccentPreset) => void
  // 设置任意 HEX 并立即切到自定义档
  setCustomAccent: (hex: string) => void
  setFontScale: (fontScale: FontScale) => void
  setUiFontSize: (size: number) => void
  setCodeFontSize: (size: number) => void
  setRadius: (radius: RadiusMode) => void
  setMotion: (motion: MotionMode) => void
  resetAppearance: () => void
}

type PersistedUiState = Pick<
  UiState,
  | 'sidebarCollapsed'
  | 'sidebarWidth'
  | 'theme'
  | 'density'
  | 'accent'
  | 'accentRevision'
  | 'customAccent'
  | 'fontScale'
  | 'uiFontSize'
  | 'codeFontSize'
  | 'radius'
  | 'motion'
>

// 快捷按钮只循环三种常用模式；旧的定时偏好仍可读取并在设置页使用。
const THEME_SEQUENCE: readonly ThemeMode[] = ['system', 'dark', 'light']
const STORED_THEME_MODES: readonly ThemeMode[] = ['system', 'scheduled', 'light', 'dark']

export function nextThemeMode(theme: ThemeMode): ThemeMode {
  return THEME_SEQUENCE[(THEME_SEQUENCE.indexOf(theme) + 1) % THEME_SEQUENCE.length]
}

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      sidebarCollapsed: false,
      sidebarWidth: DEFAULT_SIDEBAR_WIDTH,
      accentRevision: ACCENT_REVISION,
      ...DEFAULT_UI_PREFERENCES,
      setSidebarCollapsed: (sidebarCollapsed) => set({ sidebarCollapsed }),
      setSidebarWidth: (sidebarWidth) => set({ sidebarWidth: clampSidebarWidth(sidebarWidth) }),
      toggleSidebar: () => set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed })),
      setTheme: (theme) => set({ theme }),
      cycleTheme: () =>
        set((state) => ({
          theme: nextThemeMode(state.theme),
        })),
      setDensity: (density) => set({ density }),
      setAccent: (accent) => set({ accent }),
      setCustomAccent: (customAccent) => set({ accent: 'custom', customAccent }),
      setFontScale: (fontScale) => set({ fontScale, uiFontSize: LEGACY_FONT_SIZES[fontScale] }),
      setUiFontSize: (size) => {
        const { uiFontSize } = normalizeTypographyPreferences({ uiFontSize: size })
        set({ uiFontSize, fontScale: legacyFontScale(uiFontSize) })
      },
      setCodeFontSize: (size) =>
        set({ codeFontSize: normalizeTypographyPreferences({ codeFontSize: size }).codeFontSize }),
      setRadius: (radius) => set({ radius }),
      setMotion: (motion) => set({ motion }),
      resetAppearance: () => set(DEFAULT_UI_PREFERENCES),
    }),
    {
      name: 'pisper-ui',
      storage: createJSONStorage(() => pageStateStorage),
      version: 2,
      migrate: (persisted, version): PersistedUiState => {
        const stored = persisted as Partial<PersistedUiState>
        return {
          sidebarCollapsed: stored.sidebarCollapsed ?? false,
          sidebarWidth: clampSidebarWidth(stored.sidebarWidth),
          ...DEFAULT_UI_PREFERENCES,
          ...stored,
          ...normalizeTypographyPreferences(stored),
          // 保留旧修订号（缺省为 0），由 merge 统一执行强调色迁移。
          accentRevision: Number(stored.accentRevision) || 0,
          theme:
            version === 0 && stored.theme === 'system' ? 'scheduled' : (stored.theme ?? 'system'),
        }
      },
      partialize: ({
        sidebarCollapsed,
        sidebarWidth,
        theme,
        density,
        accent,
        accentRevision,
        customAccent,
        fontScale,
        uiFontSize,
        codeFontSize,
        radius,
        motion,
      }) => ({
        sidebarCollapsed,
        sidebarWidth,
        theme,
        density,
        accent,
        accentRevision,
        customAccent,
        fontScale,
        uiFontSize,
        codeFontSize,
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
              : STORED_THEME_MODES.includes(legacyTheme as ThemeMode)
                ? (legacyTheme as ThemeMode)
                : current.theme),
          sidebarCollapsed: stored?.sidebarCollapsed ?? legacySidebar === '1',
          sidebarWidth: clampSidebarWidth(stored?.sidebarWidth),
          density: stored?.density ?? (legacyDensity === 'compact' ? 'compact' : current.density),
          accent: migrateStoredAccent(stored?.accent, stored?.accentRevision) ?? current.accent,
          accentRevision: ACCENT_REVISION,
          customAccent: stored?.customAccent ?? current.customAccent,
          ...normalizeTypographyPreferences(stored),
          fontScale: legacyFontScale(normalizeTypographyPreferences(stored).uiFontSize),
          radius: stored?.radius ?? current.radius,
          motion: stored?.motion ?? current.motion,
        }
      },
    },
  ),
)
