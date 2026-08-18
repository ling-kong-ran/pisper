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

// 按页面 id 取路径；未知 id 回退到聊天页，避免调用方硬编码。
export function pagePath(page: string) {
  return PAGE_PATHS[page as PageId] || PAGE_PATHS.chat
}

// 路径 → 页面 id：先归一化尾部斜杠，再单独处理动态段
// （/workflows/:id 与 /config/:section），其余查静态映射表。
export function pageFromPath(pathname: string): PageId | null {
  const normalized = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname
  if (/^\/workflows\/[^/]+$/.test(normalized)) return 'workflowCreate'
  if (/^\/config(?:\/[^/]+)?$/.test(normalized)) return 'config'
  return PATH_PAGES.get(normalized) || null
}

// 工作流编辑器路径（默认新建）；id 做 URL 编码防止特殊字符破坏路由。
export function workflowPath(id = 'new') {
  return `/workflows/${encodeURIComponent(id || 'new')}`
}

// 旧版“#页面名”哈希路径迁移：命中已知页面 id 时映射到新路径，
// 否则返回 null（保持现有路径不动），供入口启动时替换一次。
export function legacyHashPath(hash: string): string | null {
  const legacyPage = String(hash || '').replace(/^#/, '')
  return PAGE_IDS.has(legacyPage) ? pagePath(legacyPage) : null
}
