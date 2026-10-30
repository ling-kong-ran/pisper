// 会话主页（'/'）的落地判定与“上次停留的会话”偏好。只随会话路由懒加载，不进入入口包。
// 主页本身从不建会话：上次停留的会话还在且未归档时以 replace 打开它；不存在、已归档或已删除时显示
// “新任务”草稿；会话目录还在加载时等待（不闪草稿、不先跳看板）；目录读取失败且没有可用快照时给出错误态。
import { pageStateStorage } from '@/lib/page-state-storage'

// 本设备最近一次打开的真实会话 id（不含 '/board/new'）。登记在 shared/browser-preferences.mjs：
// 偏好镜像只走本机 Runtime 的 /api/local（远程监听返回 404），所以它天然按设备记，不跨设备同步。
export const LAST_SESSION_KEY = 'pisper-last-session'
// 会话 id 是 Runtime 生成的短标识；超长或带控制字符的值一律视为无效，不拿去拼路由。
export const LAST_SESSION_ID_MAX_LENGTH = 256

// 存储里的值按不可信输入处理：只接受去掉首尾空白后非空、限长、不含控制字符的字符串。
export function parseLastSessionId(value: unknown): string {
  if (typeof value !== 'string') return ''
  const id = value.trim()
  if (!id || id.length > LAST_SESSION_ID_MAX_LENGTH || id === 'new') return ''
  // eslint-disable-next-line no-control-regex
  return /[\u0000-\u001f\u007f]/.test(id) ? '' : id
}

export function readLastSession(): string {
  try {
    return parseLastSessionId(pageStateStorage.getItem(LAST_SESSION_KEY))
  } catch {
    // 存储不可读（隐私模式、被禁用）时按“没有上次会话”处理：主页显示草稿。
    return ''
  }
}

export function rememberLastSession(id: string) {
  const value = parseLastSessionId(id)
  if (!value) return
  try {
    if (pageStateStorage.getItem(LAST_SESSION_KEY) !== value)
      pageStateStorage.setItem(LAST_SESSION_KEY, value)
  } catch {
    // 写不进去只影响下次落地，不影响当前会话。
  }
}

// 归档当前会话后清掉记录：主页随后显示草稿，不会因为目录快照还没刷新又落回刚归档的会话。
export function forgetLastSession(id: string) {
  try {
    if (pageStateStorage.getItem(LAST_SESSION_KEY) === id)
      pageStateStorage.removeItem(LAST_SESSION_KEY)
  } catch {
    // 同上：存储不可用时主页按目录里的归档状态判断。
  }
}

export type HomeLanding =
  { kind: 'draft' } | { kind: 'loading' } | { kind: 'error' } | { kind: 'session'; id: string }

type CatalogEntry = { id: string; archived: boolean }

// 会话目录（/api/sessions 的快照）按不可信数据读取：只取带字符串 id 的条目。
function catalogEntries(data: unknown): CatalogEntry[] | null {
  if (!data || typeof data !== 'object') return null
  const sessions = (data as { sessions?: unknown }).sessions
  if (!Array.isArray(sessions)) return null
  return sessions.flatMap((value) => {
    if (!value || typeof value !== 'object') return []
    const { id, archived } = value as { id?: unknown; archived?: unknown }
    return typeof id === 'string' && id ? [{ id, archived: archived === true }] : []
  })
}

export type HomeLandingInput = {
  lastId: string
  // 会话目录查询的数据（可能是失败前留下的旧快照）。
  data: unknown
  // 查询已失败且当前没有在重试。
  failed: boolean
}

export function homeLanding({ lastId, data, failed }: HomeLandingInput): HomeLanding {
  // 没有上次会话：不必等目录，直接给草稿。
  if (!lastId) return { kind: 'draft' }
  const entries = catalogEntries(data)
  if (!entries) return failed ? { kind: 'error' } : { kind: 'loading' }
  const entry = entries.find((item) => item.id === lastId)
  return entry && !entry.archived ? { kind: 'session', id: entry.id } : { kind: 'draft' }
}
