// UI 偏好统一写入根元素,保证 React 组件、第三方控件和纯 CSS 表面读取同一状态。
import { customAccentStyleRules, normalizeHexColor } from '@/lib/custom-accent'
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
}

const CUSTOM_ACCENT_STYLE_ID = 'pisper-custom-accent-style'

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
