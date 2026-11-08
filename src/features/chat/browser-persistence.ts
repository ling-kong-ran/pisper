// 右侧浏览器落盘前的清洗，以及每个会话打开的网页（页签）的序列化与解析。
// 页签和浏览历史都经 page-state-storage 写进本机偏好文件：文件明文、跨端口长期保留，
// 所以只留能重新打开页面的最少信息，凭据样的参数、片段和应用自己的带令牌地址一律不写。
import type { ContextPages } from './session-context-pages'

export const BROWSER_URL_MAX_LENGTH = 2048
export const BROWSER_TITLE_MAX_LENGTH = 200
export const PERSISTED_BROWSER_SESSIONS = 40
export const PERSISTED_BROWSER_TABS = 12
// 单个偏好值的上限是 512 KiB（shared/browser-preferences.mjs），总量 2 MiB；留足余量给别的偏好。
export const PERSISTED_BROWSER_TABS_MAX_BYTES = 256 * 1024
const SESSION_ID_MAX_LENGTH = 200

// 参数名按 -_. 拆开后任一段命中就丢；再按子串认几个常见的拼在一起的写法（sessionid、apikey、id_token）。
const SECRET_PARAM_PARTS = new Set([
  'token',
  'code',
  'secret',
  'password',
  'passwd',
  'key',
  'apikey',
  'sig',
  'signature',
  'session',
  'auth',
  'authorization',
  'ticket',
  'otp',
])
// auth 不按子串匹配：author 这类普通参数不能被误删。
const SECRET_PARAM_SUBSTRING = /token|secret|passw|apikey|signature|session/

export function isSecretQueryParam(name: string) {
  const lower = name.toLowerCase()
  if (SECRET_PARAM_SUBSTRING.test(lower)) return true
  return lower.split(/[-_.[\]]+/).some((part) => SECRET_PARAM_PARTS.has(part))
}

// 应用自己的地址（工作区 HTML 预览 /_pisper/html-preview/<令牌>/、桌面引导 /_pisper/desktop/bootstrap?token=）
// 带一次性令牌，重启后也失效；和应用同源的其他页面同理（随机端口，API 带会话），都不保存。
function isAppUrl(url: URL, appOrigin: string | undefined) {
  if (url.pathname.startsWith('/_pisper/')) return true
  if (!appOrigin) return false
  try {
    return new URL(appOrigin).origin === url.origin
  } catch {
    return false
  }
}

// 不能保存时返回 null。输入按不可信处理：非字符串、超长、非 http(s) 都丢。
export function sanitizeBrowserUrl(value: unknown, appOrigin?: string): string | null {
  if (typeof value !== 'string') return null
  const text = value.trim()
  if (!text || text.length > BROWSER_URL_MAX_LENGTH * 2) return null
  let url: URL
  try {
    url = new URL(text)
  } catch {
    return null
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return null
  if (isAppUrl(url, appOrigin)) return null
  url.username = ''
  url.password = ''
  url.hash = ''
  const secret = [...url.searchParams.keys()].filter(isSecretQueryParam)
  // 只在真有要删的参数时改写查询串：URLSearchParams 重新编码会把 %20 写成 +。
  for (const name of new Set(secret)) url.searchParams.delete(name)
  const href = url.href
  return href.length <= BROWSER_URL_MAX_LENGTH ? href : null
}

// 标题和会话 id 来自网页与存储文件，控制字符一律不要（换行会打乱标签显示）。
// eslint-disable-next-line no-control-regex
const CONTROL_CHARS = /[\u0000-\u001f\u007f]+/g

export function sanitizeBrowserTitle(value: unknown) {
  if (typeof value !== 'string') return ''
  return value.replace(CONTROL_CHARS, ' ').trim().slice(0, BROWSER_TITLE_MAX_LENGTH)
}

export type PersistedBrowserTab = { url: string; title: string }
export type PersistedBrowserSession = {
  id: string
  updatedAt: number
  // tabs 里当前显示的那一个。
  active: number
  tabs: PersistedBrowserTab[]
}

// 一个会话要保存的页签：只有浏览器页签，空白页和不能保存的地址跳过。没有可存的就返回 null。
export function browserTabsSnapshot(
  state: ContextPages,
  appOrigin?: string,
): Pick<PersistedBrowserSession, 'active' | 'tabs'> | null {
  const tabs: PersistedBrowserTab[] = []
  let active = 0
  for (const page of state.pages) {
    if (page.kind !== 'browser' || tabs.length >= PERSISTED_BROWSER_TABS) continue
    const url = sanitizeBrowserUrl(page.url, appOrigin)
    if (!url) continue
    if (page.id === state.activeBrowserId) active = tabs.length
    // 地址被清洗过时原标题可能对应带参数的那一页，仍然保留：只用于标签显示。
    tabs.push({ url, title: sanitizeBrowserTitle(page.title) })
  }
  return tabs.length ? { active, tabs } : null
}

function validSessionId(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    value.length > 0 &&
    value.length <= SESSION_ID_MAX_LENGTH &&
    value.replace(CONTROL_CHARS, '') === value
  )
}

function parseSession(value: unknown): PersistedBrowserSession | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const source = value as Record<string, unknown>
  if (!validSessionId(source.id) || !Array.isArray(source.tabs)) return null
  const updatedAt =
    typeof source.updatedAt === 'number' && Number.isSafeInteger(source.updatedAt)
      ? Math.max(0, source.updatedAt)
      : 0
  const tabs: PersistedBrowserTab[] = []
  let active = 0
  const wanted =
    typeof source.active === 'number' && Number.isSafeInteger(source.active) ? source.active : 0
  source.tabs.forEach((tab: unknown, index) => {
    if (tabs.length >= PERSISTED_BROWSER_TABS || !tab || typeof tab !== 'object') return
    const item = tab as Record<string, unknown>
    // 读回时再清洗一次：文件可能是旧版本或被改过。
    const url = sanitizeBrowserUrl(item.url)
    if (!url) return
    if (index === wanted) active = tabs.length
    tabs.push({ url, title: sanitizeBrowserTitle(item.title) })
  })
  return tabs.length ? { id: source.id, updatedAt, active, tabs } : null
}

// 读不出来（缺失、损坏、版本不对）就当没有，不影响启动。
export function parseBrowserTabs(raw: string | null | undefined): PersistedBrowserSession[] {
  if (!raw) return []
  let value: unknown
  try {
    value = JSON.parse(raw)
  } catch {
    return []
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return []
  const source = value as Record<string, unknown>
  if (source.version !== 1 || !Array.isArray(source.sessions)) return []
  const seen = new Set<string>()
  const sessions: PersistedBrowserSession[] = []
  for (const item of source.sessions.slice(0, PERSISTED_BROWSER_SESSIONS * 2)) {
    const session = parseSession(item)
    if (!session || seen.has(session.id)) continue
    seen.add(session.id)
    sessions.push(session)
  }
  return sessions.sort((a, b) => b.updatedAt - a.updatedAt).slice(0, PERSISTED_BROWSER_SESSIONS)
}

function byteLength(text: string) {
  return new TextEncoder().encode(text).byteLength
}

// 最近改过的会话在前，最多 40 个；超出字节预算时从最旧的开始丢。没有可存的返回 null（删掉这个键）。
export function serializeBrowserTabs(sessions: PersistedBrowserSession[]): string | null {
  const kept = [...sessions]
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .slice(0, PERSISTED_BROWSER_SESSIONS)
  while (kept.length) {
    const text = JSON.stringify({ version: 1, sessions: kept })
    if (byteLength(text) <= PERSISTED_BROWSER_TABS_MAX_BYTES) return text
    kept.pop()
  }
  return null
}
