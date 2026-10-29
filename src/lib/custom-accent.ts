// 自定义强调色:从单个 HEX 派生整套 accent 设计 token。
// 变量面对齐 index.css 中各预设 accent 块(含暗色主题的增亮策略与 --accent-fg 文字色),
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
  if (contrastRatio(base, WHITE) >= 4.5) return '#ffffff'
  if (contrastRatio(base, INK) >= 4.5) return '#18181b'
  // 中间亮度颜色与墨色可能仍不足 4.5；纯黑补足该区间，不修改用户选色。
  return '#000000'
}

function readableHover(base: Rgb, foreground: string, amount: number) {
  const ink = hexToRgb(foreground)
  const target = contrastRatio(ink, WHITE) > contrastRatio(ink, INK) ? WHITE : INK
  return mix(base, target, amount)
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

// 运行时注入不会经过 CSS 构建降级；直接生成 RGBA，保留旧 WebView 中的透明度。
export function customAccentStyleRules(hex: string) {
  const base = hexToRgb(hex)
  const on = onAccent(base)
  const hover = readableHover(base, on, 0.12)
  const strong = strongOnLightSurface(base)
  const soft = mix(base, WHITE, 0.93)
  const border = mix(base, WHITE, 0.72)
  const darkBase = mix(base, WHITE, 0.22)
  const darkStrong = strongOnDarkSurface(base)
  const darkBaseRgb = hexToRgb(darkBase)
  const darkOn = onAccent(darkBaseRgb)
  const darkHover = readableHover(darkBaseRgb, darkOn, 0.14)
  return `:root[data-accent='custom'] {
  --star: ${hex};
  --star-hover: ${hover};
  --star-strong: ${strong};
  --star-soft: ${soft};
  --star-border: ${border};
  --on-accent: ${on};
  --accent-fg: ${strong};
  --brand-blue: ${hex};
  --brand-blue-hover: ${hover};
  --brand-blue-strong: ${strong};
  --brand-blue-soft: ${soft};
  --brand-blue-border: ${border};
  --accent-ring: ${alpha(base, 0.16)};
  --focus: ${strong};
  --focus-ring: ${alpha(base, 0.18)};
}
:root[data-theme='dark'][data-accent='custom'] {
  --star: ${darkBase};
  --star-hover: ${darkHover};
  --star-strong: ${darkStrong};
  --star-soft: ${alpha(darkBaseRgb, 0.15)};
  --star-border: ${alpha(darkBaseRgb, 0.38)};
  --on-accent: ${darkOn};
  --accent-fg: ${darkStrong};
  --brand-blue: ${darkBase};
  --brand-blue-hover: ${darkHover};
  --brand-blue-strong: ${darkStrong};
  --brand-blue-soft: ${alpha(darkBaseRgb, 0.15)};
  --brand-blue-border: ${alpha(darkBaseRgb, 0.34)};
  --accent-ring: ${alpha(darkBaseRgb, 0.22)};
  --focus: ${darkStrong};
  --focus-ring: ${alpha(darkBaseRgb, 0.26)};
}`
}
