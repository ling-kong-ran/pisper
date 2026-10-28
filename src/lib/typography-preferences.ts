export const UI_FONT_SIZE = { default: 14, min: 11, max: 16 } as const
export const CODE_FONT_SIZE = { default: 12, min: 8, max: 24 } as const
export const LEGACY_FONT_SIZES = { small: 13.5, default: 14, large: 14.5 } as const

export type TypographyPreferences = { uiFontSize: number; codeFontSize: number }

function boundedSize(value: unknown, range: { default: number; min: number; max: number }) {
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.min(range.max, Math.max(range.min, value))
    : range.default
}

// 旧三档只负责迁移；已有数值是权威来源，不能被兼容投影反向覆盖。
export function normalizeTypographyPreferences(value: unknown): TypographyPreferences {
  const stored = value && typeof value === 'object' ? value : {}
  const legacy = 'fontScale' in stored ? stored.fontScale : undefined
  const legacySize =
    typeof legacy === 'string' && Object.hasOwn(LEGACY_FONT_SIZES, legacy)
      ? LEGACY_FONT_SIZES[legacy as keyof typeof LEGACY_FONT_SIZES]
      : UI_FONT_SIZE.default
  return {
    uiFontSize: boundedSize('uiFontSize' in stored ? stored.uiFontSize : legacySize, UI_FONT_SIZE),
    codeFontSize: boundedSize(
      'codeFontSize' in stored ? stored.codeFontSize : undefined,
      CODE_FONT_SIZE,
    ),
  }
}

export function legacyFontScale(size: number): keyof typeof LEGACY_FONT_SIZES {
  return size < UI_FONT_SIZE.default ? 'small' : size > UI_FONT_SIZE.default ? 'large' : 'default'
}

export function parseFontSizeDraft(value: string, role: 'ui' | 'code'): number | null {
  if (!/^\d+$/.test(value.trim())) return null
  const size = Number(value)
  const range = role === 'ui' ? UI_FONT_SIZE : CODE_FONT_SIZE
  return Number.isInteger(size) && size >= range.min && size <= range.max ? size : null
}
