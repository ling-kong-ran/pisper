// 时间词解析：把“上周”“3 天前”“2026-10”变成时间窗口。规则解析不调 LLM，因为每轮自动回忆都要走。
// 只解析得足够用：解析错了也只是窗口外的结果被降权，不会把记忆过滤掉。
const DAY_MS = 86_400_000

const startOfDay = (date) => new Date(date.getFullYear(), date.getMonth(), date.getDate())
const addDays = (date, days) => new Date(date.getFullYear(), date.getMonth(), date.getDate() + days)
const startOfWeek = (date) => addDays(startOfDay(date), -((date.getDay() + 6) % 7))
const window = (since, until, label) => ({
  since: since.toISOString(),
  until: until.toISOString(),
  label,
})

const CHINESE_NUMBERS = {
  一: 1,
  两: 2,
  二: 2,
  三: 3,
  四: 4,
  五: 5,
  六: 6,
  七: 7,
  八: 8,
  九: 9,
  十: 10,
}
const parseCount = (value) => (/^\d+$/.test(value) ? Number(value) : CHINESE_NUMBERS[value] || 0)

// 返回 { since, until, label }（ISO 字符串，until 为开区间），没有时间词时返回 null。
export function parseTimeExpression(text, now = Date.now()) {
  const value = String(text || '')
  const today = startOfDay(new Date(now))
  const tomorrow = addDays(today, 1)

  let match = value.match(/(\d{4})-(\d{1,2})-(\d{1,2})/)
  if (match) {
    const since = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]))
    return window(since, addDays(since, 1), match[0])
  }
  match = value.match(/(\d{4})\s*(?:年|-)\s*(\d{1,2})\s*月?/)
  if (match) {
    const since = new Date(Number(match[1]), Number(match[2]) - 1, 1)
    return window(since, new Date(Number(match[1]), Number(match[2]), 1), match[0])
  }
  match = value.match(/(\d{1,2})\s*月\s*(\d{1,2})\s*[日号]/)
  if (match) {
    const since = new Date(today.getFullYear(), Number(match[1]) - 1, Number(match[2]))
    return window(since, addDays(since, 1), match[0])
  }
  // “N 周前”要先于“上周”，否则“两周前”会被当成别的。
  match = value.match(/(\d+|[一两二三四五六七八九十])\s*(?:天|日)前/)
  if (match) {
    const days = parseCount(match[1])
    return window(addDays(today, -days - 1), addDays(today, -days + 2), match[0])
  }
  match = value.match(/(\d+)\s*days?\s+ago/i)
  if (match) {
    const days = Number(match[1])
    return window(addDays(today, -days - 1), addDays(today, -days + 2), match[0])
  }
  match = value.match(/(\d+|[一两二三四五六七八九十])\s*(?:周|星期|个星期)前/)
  if (match) {
    const weeks = parseCount(match[1])
    const start = addDays(startOfWeek(today), -7 * weeks)
    return window(start, addDays(start, 7), match[0])
  }
  match = value.match(
    /(?:最近|过去|近|past|last)\s*(\d+|[一两二三四五六七八九十])\s*(?:天|日|days?)/i,
  )
  if (match) return window(addDays(today, -parseCount(match[1])), tomorrow, match[0])

  if (/前天/.test(value)) return window(addDays(today, -2), addDays(today, -1), '前天')
  if (/昨天|昨日|yesterday/i.test(value)) return window(addDays(today, -1), today, '昨天')
  if (/今天|今日|today/i.test(value)) return window(today, tomorrow, '今天')
  if (/上周|上星期|上个星期|last week/i.test(value)) {
    const start = addDays(startOfWeek(today), -7)
    return window(start, addDays(start, 7), '上周')
  }
  if (/这周|本周|这星期|这个星期|this week/i.test(value))
    return window(startOfWeek(today), tomorrow, '本周')
  if (/上个?月|last month/i.test(value)) {
    const since = new Date(today.getFullYear(), today.getMonth() - 1, 1)
    return window(since, new Date(today.getFullYear(), today.getMonth(), 1), '上个月')
  }
  if (/这个月|本月|this month/i.test(value))
    return window(new Date(today.getFullYear(), today.getMonth(), 1), tomorrow, '本月')
  if (/去年|last year/i.test(value))
    return window(
      new Date(today.getFullYear() - 1, 0, 1),
      new Date(today.getFullYear(), 0, 1),
      '去年',
    )
  if (/今年|this year/i.test(value))
    return window(new Date(today.getFullYear(), 0, 1), tomorrow, '今年')
  if (/最近|近期|recently|lately/i.test(value))
    return window(new Date(today.getTime() - 14 * DAY_MS), tomorrow, '最近')
  return null
}

// 问的是过去的状态（“之前用的什么”“原来是怎么定的”）：被替换的旧事实也要进结果。
export function hasHistoryIntent(text) {
  return /之前|以前|原来|当时|曾经|过去|改之前|历史|used to|previously|formerly|back then|in the past|history/iu.test(
    String(text || ''),
  )
}

// 问的是这段时间发生了什么（“上周干了什么”），而不是某个具体事实：摘要层优先。
export function hasSummaryIntent(text) {
  return /(?:干了|做了|发生了?|忙了?|改了|修了|进展|总结|回顾|what (?:did|have|happened|changed)|summary|recap)/iu.test(
    String(text || ''),
  )
}

// 给模型看的相对时间：20d、3mo、2y。
export function ageLabel(at, now = Date.now()) {
  const ms = now - Date.parse(at)
  if (!Number.isFinite(ms) || ms < 0) return '0d'
  const days = Math.floor(ms / DAY_MS)
  if (days < 60) return `${days}d`
  if (days < 730) return `${Math.floor(days / 30)}mo`
  return `${Math.floor(days / 365)}y`
}
