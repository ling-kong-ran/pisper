// 时段和字号与 ZCode 草稿页一致；纯函数便于验证边界和窄屏行为。
export function greetingPeriod(date: Date): number {
  const hour = date.getHours()
  if (hour >= 5 && hour < 9) return 0
  if (hour >= 9 && hour < 12) return 1
  if (hour >= 12 && hour < 14) return 2
  if (hour >= 14 && hour < 18) return 3
  if (hour >= 18 && hour < 23) return 4
  return 5
}

export function nextGreetingDelay(date: Date): number {
  for (const hour of [5, 9, 12, 14, 18, 23, 29]) {
    const next = new Date(date)
    next.setHours(hour, 0, 0, 0)
    if (next.getTime() > date.getTime()) return next.getTime() - date.getTime()
  }
  return 60_000
}

export function greetingFontSize(available: number, natural: number): number {
  if (!Number.isFinite(available) || !Number.isFinite(natural) || available <= 0 || natural <= 0)
    return 30
  return Math.max(20, Math.min(30, Math.floor((30 * available) / natural)))
}
