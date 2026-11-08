// 右侧浏览器的浏览历史（本设备）：同一地址只留一条，记最后访问时间和次数，最近的在前。
// 纯函数，Store 在 browser-history-store.ts；地址清洗与页签落盘共用 browser-persistence.ts。
import {
  BROWSER_TITLE_MAX_LENGTH,
  sanitizeBrowserTitle,
  sanitizeBrowserUrl,
} from './browser-persistence'

export const MAX_BROWSER_HISTORY = 500
// 和页签一样留在单个偏好值的上限（512 KiB）之内。
export const BROWSER_HISTORY_MAX_BYTES = 320 * 1024
const MAX_VISITS = 1_000_000

export type BrowserHistoryEntry = {
  url: string
  title: string
  lastVisitedAt: number
  visits: number
}

// 记一次访问。不能保存的地址（about:blank、应用自己的预览地址等）原样返回，调用方据此判断没变。
export function recordBrowserVisit(
  entries: BrowserHistoryEntry[],
  rawUrl: string,
  rawTitle: string,
  now: number,
  appOrigin?: string,
): BrowserHistoryEntry[] {
  const url = sanitizeBrowserUrl(rawUrl, appOrigin)
  if (!url) return entries
  const previous = entries.find((entry) => entry.url === url)
  const title = sanitizeBrowserTitle(rawTitle) || previous?.title || ''
  const entry = {
    url,
    title,
    lastVisitedAt: Math.max(now, previous?.lastVisitedAt ?? 0),
    visits: Math.min(MAX_VISITS, (previous?.visits ?? 0) + 1),
  }
  return [entry, ...entries.filter((item) => item.url !== url)].slice(0, MAX_BROWSER_HISTORY)
}

// 标题晚于导航到达：只补到对应那一条上，不算一次访问。
export function updateBrowserHistoryTitle(
  entries: BrowserHistoryEntry[],
  rawUrl: string,
  rawTitle: string,
  appOrigin?: string,
): BrowserHistoryEntry[] {
  const url = sanitizeBrowserUrl(rawUrl, appOrigin)
  const title = sanitizeBrowserTitle(rawTitle)
  if (!url || !title) return entries
  const index = entries.findIndex((entry) => entry.url === url)
  if (index < 0 || entries[index].title === title) return entries
  return entries.map((entry, i) => (i === index ? { ...entry, title } : entry))
}

function parseEntry(value: unknown): BrowserHistoryEntry | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const source = value as Record<string, unknown>
  const url = sanitizeBrowserUrl(source.url)
  if (!url) return null
  const lastVisitedAt =
    typeof source.lastVisitedAt === 'number' && Number.isSafeInteger(source.lastVisitedAt)
      ? Math.max(0, source.lastVisitedAt)
      : 0
  const visits =
    typeof source.visits === 'number' && Number.isSafeInteger(source.visits)
      ? Math.min(MAX_VISITS, Math.max(1, source.visits))
      : 1
  return { url, title: sanitizeBrowserTitle(source.title), lastVisitedAt, visits }
}

// 读不出来就当没有历史。清洗后可能出现重复地址（旧数据带了片段），合并成一条。
export function parseBrowserHistory(raw: string | null | undefined): BrowserHistoryEntry[] {
  if (!raw) return []
  let value: unknown
  try {
    value = JSON.parse(raw)
  } catch {
    return []
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return []
  const source = value as Record<string, unknown>
  if (source.version !== 1 || !Array.isArray(source.entries)) return []
  const byUrl = new Map<string, BrowserHistoryEntry>()
  for (const item of source.entries.slice(0, MAX_BROWSER_HISTORY * 2)) {
    const entry = parseEntry(item)
    if (!entry) continue
    const previous = byUrl.get(entry.url)
    if (!previous) byUrl.set(entry.url, entry)
    else
      byUrl.set(entry.url, {
        url: entry.url,
        title: (previous.lastVisitedAt >= entry.lastVisitedAt ? previous : entry).title,
        lastVisitedAt: Math.max(previous.lastVisitedAt, entry.lastVisitedAt),
        visits: Math.min(MAX_VISITS, previous.visits + entry.visits),
      })
  }
  return [...byUrl.values()]
    .sort((a, b) => b.lastVisitedAt - a.lastVisitedAt)
    .slice(0, MAX_BROWSER_HISTORY)
}

// 超出字节预算时从最旧的开始丢；空历史返回 null（删掉这个键）。
export function serializeBrowserHistory(entries: BrowserHistoryEntry[]): string | null {
  const kept = entries.slice(0, MAX_BROWSER_HISTORY)
  if (!kept.length) return null
  const encoder = new TextEncoder()
  let text = JSON.stringify({ version: 1, entries: kept })
  while (encoder.encode(text).byteLength > BROWSER_HISTORY_MAX_BYTES && kept.length) {
    // 每条最多约 2 KiB 地址 + 标题，一次丢一批，不必逐条重算。
    kept.splice(-Math.max(1, Math.ceil(kept.length / 10)))
    text = JSON.stringify({ version: 1, entries: kept })
  }
  return kept.length ? text : null
}

export type BrowserHistoryGroup = 'today' | 'yesterday' | 'week' | 'earlier'

// 按本地日期分组：今天、昨天、7 天内、更早。组内保持传入的顺序（Store 里已按时间倒序）。
export function groupBrowserHistory(entries: BrowserHistoryEntry[], now: number) {
  const today = new Date(now)
  today.setHours(0, 0, 0, 0)
  const startOfToday = today.getTime()
  const yesterday = new Date(today)
  yesterday.setDate(yesterday.getDate() - 1)
  const week = new Date(today)
  week.setDate(week.getDate() - 6)
  const buckets: Record<BrowserHistoryGroup, BrowserHistoryEntry[]> = {
    today: [],
    yesterday: [],
    week: [],
    earlier: [],
  }
  for (const entry of entries) {
    const key: BrowserHistoryGroup =
      entry.lastVisitedAt >= startOfToday
        ? 'today'
        : entry.lastVisitedAt >= yesterday.getTime()
          ? 'yesterday'
          : entry.lastVisitedAt >= week.getTime()
            ? 'week'
            : 'earlier'
    buckets[key].push(entry)
  }
  return (['today', 'yesterday', 'week', 'earlier'] as const)
    .filter((key) => buckets[key].length)
    .map((key) => ({ key, entries: buckets[key] }))
}

// 列表里显示的主机名；地址都已清洗过，解析失败只是兜底。
export function browserHistoryHost(url: string) {
  try {
    return new URL(url).host
  } catch {
    return url.slice(0, BROWSER_TITLE_MAX_LENGTH)
  }
}
