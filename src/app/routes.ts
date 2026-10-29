// 路由路径常量：所有页面统一从这里取路径，避免散落的硬编码字符串。
// 配置文件页复用同一 path 前缀（/config/:section），用 section 参数切换。
// pageFromPath 对动态段（workflowId、configSection、sessionId）做正则归一化后再查表，
// 供导航高亮与路由重定向使用；legacyHashPath 兼容旧版“#页面名”路径。
export const PAGE_PATHS = Object.freeze({
  // 看板是新的首页；会话页挂在 /board/:sessionId 下，'/board/new' 与 '/workflows/new' 同理表示新建。
  board: '/board',
  session: '/board/new',
  more: '/more',
  // /chat 只作兼容入口（旧链接、旧调用方的 navigate('chat')），路由层重定向到看板或会话页。
  chat: '/chat',
  chatHistory: '/chat/history',
  assets: '/assets',
  channels: '/channels',
  schedules: '/schedules',
  plugins: '/plugins',
  memory: '/memory',
  mcp: '/mcp',
  skills: '/skills',
  components: '/components',
  workflows: '/workflows',
  workflowCreate: '/workflows/new',
  customComponentTool: '/tools/components',
  config: '/config/models',
} as const)

export type PageId = keyof typeof PAGE_PATHS

export const PAGE_IDS: ReadonlySet<string> = new Set(Object.keys(PAGE_PATHS))

// 会话页的保留段：进入后创建一个新会话，再以 replace 换成真实 id。
export const NEW_SESSION_ROUTE_ID = 'new'

const PATH_PAGES = new Map<string, PageId>(
  Object.entries(PAGE_PATHS).map(([page, path]) => [path, page as PageId]),
)

// 按页面 id 取路径；未知 id 回退到看板，避免调用方硬编码。
export function pagePath(page: string) {
  return PAGE_PATHS[page as PageId] || PAGE_PATHS.board
}

// 路径 → 页面 id：先归一化尾部斜杠，再单独处理动态段
// （/board/:sessionId、/workflows/:id 与 /config/:section），其余查静态映射表。
export function pageFromPath(pathname: string): PageId | null {
  const normalized = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname
  if (/^\/board\/[^/]+$/.test(normalized)) return 'session'
  if (/^\/workflows\/[^/]+$/.test(normalized)) return 'workflowCreate'
  if (/^\/tools\/components\/[^/]+$/.test(normalized)) return 'customComponentTool'
  if (/^\/config(?:\/[^/]+)?$/.test(normalized)) return 'config'
  return PATH_PAGES.get(normalized) || null
}

// 会话页路径（默认新建）；id 做 URL 编码防止特殊字符破坏路由。
export function sessionPath(id = NEW_SESSION_ROUTE_ID) {
  return `/board/${encodeURIComponent(id || NEW_SESSION_ROUTE_ID)}`
}

// 从会话页路径取 sessionId；非会话页返回空串，非法编码同样视为空。
export function sessionIdFromPath(pathname: string) {
  const match = /^\/board\/([^/]+)\/?$/.exec(pathname)
  if (!match) return ''
  try {
    return decodeURIComponent(match[1])
  } catch {
    return ''
  }
}

// 工作流编辑器路径（默认新建）；id 做 URL 编码防止特殊字符破坏路由。
export function workflowPath(id = 'new') {
  return `/workflows/${encodeURIComponent(id || 'new')}`
}

// 旧版“#页面名”哈希路径迁移：命中已知页面 id 时映射到新路径，
// 否则返回 null（保持现有路径不动），供入口启动时替换一次。
// 旧的 #chat 仍映射到 /chat，由路由层统一重定向到看板；#session 不是旧版页面，
// 不能借它在启动时悄悄新建会话。
export function legacyHashPath(hash: string): string | null {
  const legacyPage = String(hash || '').replace(/^#/, '')
  if (legacyPage === 'session') return null
  return PAGE_IDS.has(legacyPage) ? pagePath(legacyPage) : null
}
