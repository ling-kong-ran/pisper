// 全局 UI 偏好通过 pageStateStorage 持久化；迁移层保留旧版按时间自动主题的行为，
// 避免升级后用户在相同环境下看到意外的明暗变化。
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import { STORAGE_KEYS } from '@/app/storage'
import { pageStateStorage } from '@/lib/page-state-storage'
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

// 新安装默认强调色为「石墨」（neutral，墨色主操作），琥珀只留给「需要你」。
// 已持久化的强调色不迁移：store 无法区分「默认的 blue」和「主动选的 blue」。
export const DEFAULT_UI_PREFERENCES = {
  theme: 'system',
  density: 'comfortable',
  accent: 'neutral',
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

// 侧栏收起状态不在这里：v2 侧栏由 src/stores/sidebar-store.ts（'pisper-sidebar'）独占，
// 旧侧栏的 sidebarCollapsed / sidebarWidth 已随旧外壳退役。
type UiState = {
  theme: ThemeMode
  density: DensityMode
  accent: AccentPreset
  customAccent: string
  fontScale: FontScale
  uiFontSize: number
  codeFontSize: number
  radius: RadiusMode
  motion: MotionMode
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
  | 'theme'
  | 'density'
  | 'accent'
  | 'customAccent'
  | 'fontScale'
  | 'uiFontSize'
  | 'codeFontSize'
  | 'radius'
  | 'motion'
>

// v3 迁移时的默认强调色；只用于迁移旧数据，新安装使用 DEFAULT_UI_PREFERENCES.accent。
const V3_DEFAULT_ACCENT: AccentPreset = 'blue'

// 快捷按钮只循环三种常用模式；旧的定时偏好仍可读取并在设置页使用。
const THEME_SEQUENCE: readonly ThemeMode[] = ['system', 'dark', 'light']
const STORED_THEME_MODES: readonly ThemeMode[] = ['system', 'scheduled', 'light', 'dark']

export function nextThemeMode(theme: ThemeMode): ThemeMode {
  return THEME_SEQUENCE[(THEME_SEQUENCE.indexOf(theme) + 1) % THEME_SEQUENCE.length]
}

// 旧快照还带着已退役旧侧栏的字段：读出时剔除，不让它们混进状态（下次写入 'pisper-ui' 时自然丢弃）。
function withoutRetiredSidebar(persisted: unknown): Partial<PersistedUiState> {
  if (!persisted || typeof persisted !== 'object' || Array.isArray(persisted)) return {}
  const {
    sidebarCollapsed: _collapsed,
    sidebarWidth: _width,
    ...rest
  } = persisted as Record<string, unknown>
  return rest as Partial<PersistedUiState>
}

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      ...DEFAULT_UI_PREFERENCES,
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
      version: 3,
      migrate: (persisted, version): PersistedUiState => {
        const stored = withoutRetiredSidebar(persisted)
        return {
          ...DEFAULT_UI_PREFERENCES,
          ...stored,
          ...normalizeTypographyPreferences(stored),
          // v3 迁移把旧中性档统一改为当时的默认 blue；之后默认值改为石墨，但这里仍按
          // v3 的结果迁移（缺省也视为 blue），保证升级用户看到的强调色不变。
          accent:
            version < 3 && (stored.accent === 'neutral' || stored.accent === undefined)
              ? V3_DEFAULT_ACCENT
              : (stored.accent ?? DEFAULT_UI_PREFERENCES.accent),
          theme:
            version === 0 && stored.theme === 'system' ? 'scheduled' : (stored.theme ?? 'system'),
        }
      },
      partialize: ({
        theme,
        density,
        accent,
        customAccent,
        fontScale,
        uiFontSize,
        codeFontSize,
        radius,
        motion,
      }) => ({
        theme,
        density,
        accent,
        customAccent,
        fontScale,
        uiFontSize,
        codeFontSize,
        radius,
        motion,
      }),
      merge: (persisted, current) => {
        const stored = persisted === undefined ? undefined : withoutRetiredSidebar(persisted)
        const legacyTheme = localStorage.getItem(STORAGE_KEYS.theme)
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
          density: stored?.density ?? (legacyDensity === 'compact' ? 'compact' : current.density),
          accent: stored?.accent ?? current.accent,
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
