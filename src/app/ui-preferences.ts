// UI 偏好统一写入根元素,保证 React 组件、第三方控件和纯 CSS 表面读取同一状态。
import { customAccentStyleRules, normalizeHexColor } from '@/lib/custom-accent'
import { normalizeTypographyPreferences } from '@/lib/typography-preferences'
import type {
  AccentPreset,
  DensityMode,
  FontScale,
  MotionMode,
  RadiusMode,
  ThemeMode,
} from '@/stores/ui-store'

export type UiPreferenceAttributes = {
  accent: AccentPreset
  density: DensityMode
  fontScale: FontScale
  motion: MotionMode
  radius: RadiusMode
  customAccent?: string
  uiFontSize?: number
  codeFontSize?: number
}

const CUSTOM_ACCENT_STYLE_ID = 'pisper-custom-accent-style'

const UI_SIZE_ROLES = ['xs', 'sm', 'base', 'lg', 'xl', '2xl', '3xl'] as const

export function uiTextSizes(scale: FontScale | number): number[] {
  const { uiFontSize } = normalizeTypographyPreferences(
    typeof scale === 'number' ? { uiFontSize: scale } : { fontScale: scale },
  )
  return [12, 13, 14, 16, 28, 36, 48].map((size) => Math.round((size * uiFontSize) / 14))
}

export function resolveDarkTheme(
  mode: ThemeMode,
  systemDark: boolean,
  hour = new Date().getHours(),
) {
  if (mode === 'dark') return true
  if (mode === 'light') return false
  if (mode === 'system') return systemDark
  return hour >= 18 || hour < 8
}

export function applyUiPreferenceAttributes(preferences: UiPreferenceAttributes) {
  const root = document.documentElement
  root.dataset.accent = preferences.accent
  root.dataset.density = preferences.density
  root.dataset.fontScale = preferences.fontScale
  root.dataset.motion = preferences.motion
  root.dataset.radius = preferences.radius
  const typography = normalizeTypographyPreferences(
    preferences.uiFontSize === undefined
      ? { fontScale: preferences.fontScale, codeFontSize: preferences.codeFontSize }
      : preferences,
  )
  root.style.setProperty('--ui-font-size', `${typography.uiFontSize}px`)
  root.style.setProperty('--font-code-size', `${typography.codeFontSize}px`)
  // 在 JS 中取整，避免依赖旧 WebView 尚不支持的 CSS round()。
  uiTextSizes(typography.uiFontSize).forEach((size, index) => {
    root.style.setProperty(`--font-ui-${UI_SIZE_ROLES[index]}`, `${size}px`)
  })

  // 自定义强调色:注入即时生成的变量规则;切回预设时移除,避免内联规则遮蔽预设块
  const customHex =
    preferences.accent === 'custom' ? normalizeHexColor(preferences.customAccent || '') : null
  let styleEl = document.getElementById(CUSTOM_ACCENT_STYLE_ID) as HTMLStyleElement | null
  if (customHex) {
    if (!styleEl) {
      styleEl = document.createElement('style')
      styleEl.id = CUSTOM_ACCENT_STYLE_ID
      document.head.append(styleEl)
    }
    styleEl.textContent = customAccentStyleRules(customHex)
  } else {
    styleEl?.remove()
  }
}
