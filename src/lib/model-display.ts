// 模型显示名：模型 ID 常带路径前缀（accounts/fireworks/models/qwen3-…、Qwen/Qwen3-…、
// openrouter/anthropic/claude-…），在芯片、列表、下拉框这类窄处按末尾截断时只剩前缀、看不出是哪个模型。
// 窄处只显示最后一段；同一列表里末段重名时往前多带一段，直到能区分。完整名称留给悬停提示与搜索。

// accounts/…/models/… 这类路径里的固定层级不帮助辨认模型，区分重名时跳过它们，显示 fireworks/qwen3-… 而不是 models/qwen3-…。
const FILLER_SEGMENTS = new Set([
  'accounts',
  'models',
  'model',
  'publishers',
  'projects',
  'locations',
])

function segments(label: string) {
  const parts = label
    .split('/')
    .map((part) => part.trim())
    .filter(Boolean)
  return parts.filter(
    (part, index) => index === parts.length - 1 || !FILLER_SEGMENTS.has(part.toLowerCase()),
  )
}

function tail(parts: string[], count: number) {
  return parts.slice(-count).join('/')
}

export function compactModelName(label: string, siblings: readonly string[] = []): string {
  const parts = segments(label)
  if (parts.length <= 1) return parts[0] ?? label
  const others = siblings.filter((item) => item !== label).map(segments)
  for (let count = 1; count < parts.length; count += 1) {
    const candidate = tail(parts, count)
    if (!others.some((other) => tail(other, count) === candidate)) return candidate
  }
  return parts.join('/')
}
