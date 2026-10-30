// @public 登记的项目（空项目）的纯规则：目录的规范化、按不可信数据校验、登记 / 取消登记，
// 以及与会话目录里的项目按 workspaceKey 合并去重。与 session-workspaces 分开放，是因为后者在
// 首屏依赖闭包里，而这些规则只有看板、启动台与管理项目（都是懒加载）才用得到。
import { NO_WORKSPACE_KEY, workspaceKey } from './session-workspaces'

// 登记的项目（空项目）只是本机偏好：条数与单条长度都要有界。50 条 × 2048 字符按 UTF-8 最坏
// 3 字节算约 300 KB，低于浏览器偏好单项 512 KB 的上限，写入不会被 Runtime 拒绝后无限重试。
export const REGISTERED_PROJECT_LIMIT = 50
export const REGISTERED_PROJECT_PATH_MAX_LENGTH = 2048

/**
 * 登记目录的保存形式：去掉首尾空白和末尾分隔符（根目录原样保留），不改大小写与分隔符，
 * 这样它仍能原样作为派发的 cwd；去重另按 workspaceKey 比较。非法输入返回空串。
 */
export function registeredProjectPath(value: unknown): string {
  if (typeof value !== 'string') return ''
  const trimmed = value.trim()
  if (!trimmed || trimmed.length > REGISTERED_PROJECT_PATH_MAX_LENGTH || trimmed.includes('\0'))
    return ''
  const path = /^(?:[A-Za-z]:)?[\\/]+$/u.test(trimmed) ? trimmed : trimmed.replace(/[\\/]+$/u, '')
  const key = workspaceKey(path)
  return key && key !== NO_WORKSPACE_KEY ? path : ''
}

// 按不可信数据读取：丢掉非字符串、空串和超长项，按 workspaceKey 去重（保留先出现的），限制条数。
export function normalizeRegisteredProjects(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  const seen = new Set<string>()
  const projects: string[] = []
  for (const item of value) {
    const path = registeredProjectPath(item)
    const key = path && workspaceKey(path)
    if (!key || seen.has(key)) continue
    seen.add(key)
    projects.push(path)
    if (projects.length >= REGISTERED_PROJECT_LIMIT) break
  }
  return projects
}

// 登记一个目录：最近登记的排最前（重复登记只是挪到最前），超过上限时丢掉最早登记的。
export function registerProjectPath(projects: readonly string[], cwd: unknown): string[] {
  const path = registeredProjectPath(cwd)
  if (!path) return normalizeRegisteredProjects(projects)
  const key = workspaceKey(path)
  return normalizeRegisteredProjects([
    path,
    ...projects.filter((item) => workspaceKey(item) !== key),
  ])
}

// 取消登记：按 workspaceKey 匹配，所以 /a/ 与 /a、C:\A 与 c:/a 视为同一个项目。
export function unregisterProjectPath(projects: readonly string[], cwd: string): string[] {
  const key = workspaceKey(cwd)
  return normalizeRegisteredProjects(projects.filter((item) => workspaceKey(item) !== key))
}

/**
 * 会话目录（或看板）里还没有出现的登记项目，按登记顺序（最近登记的在前）。
 * 各个项目列表把它们补进去、会话数记 0，与已有项目按同一个 workspaceKey 去重。
 */
export function unlistedRegisteredProjects(
  registered: readonly string[],
  listedKeys: Iterable<string>,
): { key: string; cwd: string }[] {
  const listed = new Set(listedKeys)
  const projects: { key: string; cwd: string }[] = []
  for (const cwd of normalizeRegisteredProjects(registered)) {
    const key = workspaceKey(cwd)
    if (listed.has(key)) continue
    listed.add(key)
    projects.push({ key, cwd })
  }
  return projects
}
