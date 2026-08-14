export const PAGE_PATHS = Object.freeze({
  chat: '/chat',
  chatHistory: '/chat/history',
  assets: '/assets',
  channels: '/channels',
  schedules: '/schedules',
  plugins: '/plugins',
  extensions: '/extensions',
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
