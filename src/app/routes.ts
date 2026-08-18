// 路由路径常量：所有页面统一从这里取路径，避免散落的硬编码字符串。
// 配置文件页复用同一 path 前缀（/config/:section），用 section 参数切换。
// pageFromPath 对动态段（workflowId、configSection）做正则归一化后再查表，
// 供导航高亮与路由重定向使用；legacyHashPath 兼容旧版“#页面名”路径。
export const PAGE_PATHS = Object.freeze({
  chat: '/chat',
  chatHistory: '/chat/history',
  assets: '/assets',
  channels: '/channels',
  schedules: '/schedules',
  plugins: '/plugins',
  memory: '/memory',
  mcp: '/mcp',
  skills: '/skills',
  workflows: '/workflows',
  workflowCreate: '/workflows/new',
  config: '/config/models',
} as const)

export type PageId = keyof typeof PAGE_PATHS

export const PAGE_IDS: ReadonlySet<string> = new Set(Object.keys(PAGE_PATHS))

const PATH_PAGES = new Map<string, PageId>(
  Object.entries(PAGE_PATHS).map(([page, path]) => [path, page as PageId]),
)

export function pagePath(page: string) {
  return PAGE_PATHS[page as PageId] || PAGE_PATHS.chat
}

export function pageFromPath(pathname: string): PageId | null {
  const normalized = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname
  if (/^\/workflows\/[^/]+$/.test(normalized)) return 'workflowCreate'
  if (/^\/config(?:\/[^/]+)?$/.test(normalized)) return 'config'
  return PATH_PAGES.get(normalized) || null
}

export function workflowPath(id = 'new') {
  return `/workflows/${encodeURIComponent(id || 'new')}`
}

export function legacyHashPath(hash: string): string | null {
  const legacyPage = String(hash || '').replace(/^#/, '')
  return PAGE_IDS.has(legacyPage) ? pagePath(legacyPage) : null
}
