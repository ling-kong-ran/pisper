// 时间线分组：今天、昨天、本周更早的几天，再往前按月。纯函数，不依赖界面。
export type MemoryTimelineGroup<T> = {
  key: string
  kind: 'today' | 'yesterday' | 'week' | 'month' | 'unknown'
  // kind 为 month 时是那个月的第一天，由调用方按语言格式化。
  month?: Date
  items: T[]
}

const startOfDay = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate())

// 一周从周一算起，和时间词解析（runtime 的 time-expression）一致。
function startOfWeek(date: Date) {
  const day = startOfDay(date)
  return new Date(day.getFullYear(), day.getMonth(), day.getDate() - ((day.getDay() + 6) % 7))
}

export function groupMemoriesByTime<T>(
  items: readonly T[],
  timeOf: (item: T) => string | undefined,
  now: number = Date.now(),
): MemoryTimelineGroup<T>[] {
  const current = new Date(now)
  const today = startOfDay(current).getTime()
  const yesterday = today - 86_400_000
  const week = startOfWeek(current).getTime()
  const groups = new Map<string, MemoryTimelineGroup<T>>()
  const add = (group: Omit<MemoryTimelineGroup<T>, 'items'>, item: T) => {
    const existing = groups.get(group.key)
    if (existing) existing.items.push(item)
    else groups.set(group.key, { ...group, items: [item] })
  }
  for (const item of items) {
    const time = Date.parse(timeOf(item) || '')
    if (Number.isNaN(time)) add({ key: 'unknown', kind: 'unknown' }, item)
    // 时钟有偏差或用户写了未来的生效时间：算今天，不单独成组。
    else if (time >= today) add({ key: 'today', kind: 'today' }, item)
    else if (time >= yesterday) add({ key: 'yesterday', kind: 'yesterday' }, item)
    else if (time >= week) add({ key: 'week', kind: 'week' }, item)
    else {
      const date = new Date(time)
      const key = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}`
      add({ key, kind: 'month', month: new Date(date.getFullYear(), date.getMonth(), 1) }, item)
    }
  }
  return [...groups.values()]
}
