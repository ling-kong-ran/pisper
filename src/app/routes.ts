// 路由路径常量：所有页面统一从这里取路径，避免散落的硬编码字符串。
// 配置文件页复用同一 path 前缀（/config/:section），用 section 参数切换。
// pageFromPath 对动态段（workflowId、configSection、sessionId、componentId）做正则归一化后再查表，
// 供导航高亮与路由重定向使用；legacyHashPath 兼容旧版“#页面名”路径。
// 主页是会话（负责人决定，取代“看板是首页”）：'/' 打开本设备上次停留的会话，没有可用会话时是“新任务”草稿；
// 主页本身不建会话。主页不是 PAGE_PATHS 里的页面 id，归属会话页（见 pageFromPath）。
export const HOME_PATH = '/'

export const PAGE_PATHS = Object.freeze({
  // 看板仍在 /board；会话页沿用 /board/:sessionId（兼容旧链接、通知与守卫），'/board/new' 与 '/workflows/new'
  // 同理表示进入即新建。
  board: '/board',
  session: '/board/new',
  more: '/more',
  // /chat 只作兼容入口（旧链接、旧调用方的 navigate('chat')），路由层重定向到主页或会话页。
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

// 手机外壳的设置分区列表（规格 7.4，列表 → 分区）：不是独立页面 id，仍归 config。
export const SETTINGS_LIST_PATH = '/config'

export type PageId = keyof typeof PAGE_PATHS

export const PAGE_IDS: ReadonlySet<string> = new Set(Object.keys(PAGE_PATHS))

// 会话页的保留段：进入后创建一个新会话，再以 replace 换成真实 id。
export const NEW_SESSION_ROUTE_ID = 'new'

const PATH_PAGES = new Map<string, PageId>(
  Object.entries(PAGE_PATHS).map(([page, path]) => [path, page as PageId]),
)

// 按页面 id 取路径；未知 id 回退到主页（会话），避免调用方硬编码。
export function pagePath(page: string) {
  return PAGE_PATHS[page as PageId] || HOME_PATH
}

// 从主页落地（或经“会话”标签回到主页）的会话页带这个历史状态：它是根，手机原生返回交还平台；
// 侧栏切换会话（replace）时随之保留。旧版“从更多进入”在同一字段写过 'activity'（双归属已退役），
// 旧历史条目里可能还留着它，按非主页落地处理。
export const HOME_ENTRY_STATE = Object.freeze({ navOrigin: 'home' })

export function isHomeEntry(state: unknown) {
  return (
    typeof state === 'object' &&
    state !== null &&
    (state as { navOrigin?: unknown }).navOrigin === HOME_ENTRY_STATE.navOrigin
  )
}

// 路径 → 页面 id：先归一化尾部斜杠，再单独处理动态段
// （/board/:sessionId、/workflows/:id 与 /config/:section），其余查静态映射表。
// '/'（主页）归会话页：标题栏与标签栏点亮“会话”，手机上与会话页同一套外壳（不显示底部标签栏）。
export function pageFromPath(pathname: string): PageId | null {
  const normalized = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname
  if (normalized === HOME_PATH) return 'session'
  if (/^\/board\/[^/]+$/.test(normalized)) return 'session'
  if (/^\/workflows\/[^/]+$/.test(normalized)) return 'workflowCreate'
  if (/^\/tools\/components\/[^/]+$/.test(normalized)) return 'customComponentTool'
  if (/^\/components\/[^/]+$/.test(normalized)) return 'components'
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

// 组件详情路径（/components/:componentId）；空 id 回到组件列表。
export function componentPath(id = '') {
  return id ? `/components/${encodeURIComponent(id)}` : '/components'
}

// 工作流编辑器路径（默认新建）；id 做 URL 编码防止特殊字符破坏路由。
export function workflowPath(id = 'new') {
  return `/workflows/${encodeURIComponent(id || 'new')}`
}

// 旧版“#页面名”哈希路径迁移：命中已知页面 id 时映射到新路径，
// 否则返回 null（保持现有路径不动），供入口启动时替换一次。
// 旧的 #chat 仍映射到 /chat，由路由层统一重定向到主页（上次的会话或新任务草稿，不建会话）；
// #session 不是旧版页面，不能借它在启动时悄悄新建会话。
export function legacyHashPath(hash: string): string | null {
  const legacyPage = String(hash || '').replace(/^#/, '')
  if (legacyPage === 'session') return null
  return PAGE_IDS.has(legacyPage) ? pagePath(legacyPage) : null
}
