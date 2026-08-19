// 自定义强调色:从单个 HEX 派生整套 accent 设计 token。
// 变量面对齐 index.css 中各预设 accent 块(含暗色主题的增亮策略),
// 由 ui-preferences 注入为即时生成的 <style> 规则。

export type Rgb = { r: number; g: number; b: number }

// 只接受 6 位 HEX;合法则规范为小写 #rrggbb,否则返回 null
export function normalizeHexColor(input: string): string | null {
  const value = String(input || '')
    .trim()
    .replace(/^#/, '')
  if (!/^[0-9a-fA-F]{6}$/.test(value)) return null
  return `#${value.toLowerCase()}`
}

function hexToRgb(hex: string): Rgb {
  return {
    r: parseInt(hex.slice(1, 3), 16),
    g: parseInt(hex.slice(3, 5), 16),
    b: parseInt(hex.slice(5, 7), 16),
  }
}

function toHex({ r, g, b }: Rgb) {
  const channel = (v: number) =>
    Math.round(Math.min(255, Math.max(0, v)))
      .toString(16)
      .padStart(2, '0')
  return `#${channel(r)}${channel(g)}${channel(b)}`
}

// 与目标色按比例混合:amount 越大越接近目标色
function mix(base: Rgb, target: Rgb, amount: number) {
  return toHex({
    r: base.r + (target.r - base.r) * amount,
    g: base.g + (target.g - base.g) * amount,
    b: base.b + (target.b - base.b) * amount,
  })
}

const WHITE: Rgb = { r: 255, g: 255, b: 255 }
const INK: Rgb = { r: 24, g: 24, b: 27 }

function alpha({ r, g, b }: Rgb, value: number) {
  return `rgba(${r}, ${g}, ${b}, ${value})`
}

// 计算 sRGB 相对亮度,为实心 accent 表面选择对比度更高的前景色
function channelLuminance(channel: number) {
  const value = channel / 255
  return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
}

// 使用 WCAG 对比度而不是单一亮度阈值,保证任意自定义色上的文字尽可能清晰
function luminance({ r, g, b }: Rgb) {
  return 0.2126 * channelLuminance(r) + 0.7152 * channelLuminance(g) + 0.0722 * channelLuminance(b)
}

function contrastRatio(first: Rgb, second: Rgb) {
  const firstLuminance = luminance(first)
  const secondLuminance = luminance(second)
  const lighter = Math.max(firstLuminance, secondLuminance)
  const darker = Math.min(firstLuminance, secondLuminance)
  return (lighter + 0.05) / (darker + 0.05)
}

function onAccent(base: Rgb) {
  return contrastRatio(base, WHITE) >= contrastRatio(base, INK) ? '#fff' : '#18181b'
}

// 强调色会出现在浅色或深色表面上的小字号文字中,不足对比度时继续向可读方向混合
function strongOnLightSurface(base: Rgb) {
  for (let amount = 0.22; amount <= 1; amount += 0.04) {
    const candidate = mix(base, INK, amount)
    if (contrastRatio(hexToRgb(candidate), WHITE) >= 4.5) return candidate
  }
  return '#18181b'
}

function strongOnDarkSurface(base: Rgb) {
  for (let amount = 0.46; amount <= 1; amount += 0.04) {
    const candidate = mix(base, WHITE, amount)
    if (contrastRatio(hexToRgb(candidate), INK) >= 4.5) return candidate
  }
  return '#fff'
}

// 生成完整 CSS 规则:亮色块复刻预设变量面,暗色块复刻预设的增亮 + color-mix 策略
export function customAccentStyleRules(hex: string) {
  const base = hexToRgb(hex)
  const hover = mix(base, INK, 0.12)
  const strong = strongOnLightSurface(base)
  const soft = mix(base, WHITE, 0.93)
  const border = mix(base, WHITE, 0.72)
  const darkBase = mix(base, WHITE, 0.22)
  const darkHover = mix(base, WHITE, 0.33)
  const darkStrong = strongOnDarkSurface(base)
  const darkBaseRgb = hexToRgb(darkBase)
  const on = onAccent(base)
  const darkOn = onAccent(darkBaseRgb)
  return `:root[data-accent='custom'] {
  --star: ${hex};
  --star-hover: ${hover};
  --star-strong: ${strong};
  --star-soft: ${soft};
  --star-border: ${border};
  --on-accent: ${on};
  --brand-blue: ${hex};
  --brand-blue-hover: ${hover};
  --brand-blue-strong: ${strong};
  --brand-blue-soft: ${soft};
  --brand-blue-border: ${border};
  --accent-ring: ${alpha(base, 0.16)};
  --focus: ${hex};
  --focus-ring: ${alpha(base, 0.18)};
}
:root[data-theme='dark'][data-accent='custom'] {
  --star: ${darkBase};
  --star-hover: ${darkHover};
  --star-strong: ${darkStrong};
  --star-soft: color-mix(in srgb, ${darkBase} 15%, transparent);
  --star-border: color-mix(in srgb, ${darkBase} 38%, transparent);
  --on-accent: ${darkOn};
  --brand-blue: ${darkBase};
  --brand-blue-hover: ${darkHover};
  --brand-blue-strong: ${darkStrong};
  --brand-blue-soft: color-mix(in srgb, ${darkBase} 15%, transparent);
  --brand-blue-border: color-mix(in srgb, ${darkBase} 34%, transparent);
  --accent-ring: ${alpha(darkBaseRgb, 0.22)};
  --focus: ${darkBase};
  --focus-ring: ${alpha(darkBaseRgb, 0.26)};
}`
}
