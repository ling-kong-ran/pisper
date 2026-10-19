import { normalizeSessionContextWidth } from './session-context-layout'
import type { ResourceReadingPosition } from './use-resource-reading'
export type SessionContextTab =
  | 'new'
  | 'changes'
  | 'files'
  | 'terminal'
  | 'processes'
  | 'sources'
  | 'plan'
  | 'browser'
  | 'side-chat'

export type ContextPageView = {
  reading: Record<string, ResourceReadingPosition>
  expandedFiles: string[]
  expandedProcesses: string[]
  processFollow: Record<string, boolean>
  processScroll: Record<string, number>
  query: string
  filter: string
  selectedSourceId: string
  follow: boolean
}
export type ContextPage = {
  id: string
  kind: SessionContextTab
  resourceId: string
  draft: string
  url: string
  view: ContextPageView
}
export type ContextPages = {
  pages: ContextPage[]
  activeId: string
  nextId: number
  contentOpen: boolean
  summaryOpen: boolean
  contentWidth: number | null
  conversationScrollTop: number | null
}
export type ContextPageAction =
  | { type: 'add'; kind: SessionContextTab; resourceId?: string }
  | { type: 'open'; kind: SessionContextTab; resourceId?: string; replaceId?: string }
  | { type: 'select'; id: string }
  | { type: 'close'; id: string }
  | { type: 'kind'; id: string; kind: SessionContextTab }
  | { type: 'browser'; id: string; draft?: string; url?: string }
  | { type: 'resource'; id: string; resourceId: string }
  | { type: 'view'; id: string; patch: Partial<ContextPageView> }
  | {
      type: 'workspace'
      contentWidth?: number
      contentOpen?: boolean
      summaryOpen?: boolean
      conversationScrollTop?: number
    }

export const MAX_CONTEXT_PAGES = 12
const SINGLETONS = new Set<SessionContextTab>([
  'changes',
  'files',
  'plan',
  'processes',
  'sources',
  'side-chat',
])

function createPage(id: string, kind: SessionContextTab, resourceId = ''): ContextPage {
  return {
    id,
    kind,
    resourceId,
    draft: '',
    url: '',
    view: {
      reading: {},
      expandedFiles: [],
      expandedProcesses: [],
      processFollow: {},
      processScroll: {},
      query: '',
      filter: '',
      selectedSourceId: '',
      follow: true,
    },
  }
}

export function createContextPages(kind?: SessionContextTab): ContextPages {
  return {
    pages: kind ? [createPage('page-1', kind)] : [],
    activeId: kind ? 'page-1' : '',
    nextId: kind ? 2 : 1,
    contentOpen: false,
    summaryOpen: false,
    contentWidth: null,
    conversationScrollTop: null,
  }
}

export function updateContextPages(state: ContextPages, action: ContextPageAction): ContextPages {
  if (action.type === 'workspace') {
    return {
      ...state,
      ...(action.contentWidth !== undefined
        ? { contentWidth: normalizeSessionContextWidth(action.contentWidth) }
        : {}),
      ...(action.contentOpen !== undefined
        ? { contentOpen: action.contentOpen && state.pages.length > 0 }
        : {}),
      ...(action.summaryOpen !== undefined ? { summaryOpen: action.summaryOpen } : {}),
      ...(action.conversationScrollTop !== undefined &&
      Number.isFinite(action.conversationScrollTop)
        ? { conversationScrollTop: Math.max(0, action.conversationScrollTop) }
        : {}),
    }
  }
  if (action.type === 'add' || action.type === 'open' || action.type === 'kind') {
    const replaceId =
      action.type === 'kind' ? action.id : action.type === 'open' ? action.replaceId : undefined
    const replaced = state.pages.find((page) => page.id === replaceId)
    if (action.type === 'kind' && !replaced) return state
    const resourceId = 'resourceId' in action ? action.resourceId || '' : ''
    const existing = state.pages.find(
      (page) =>
        page.id !== replaceId &&
        page.kind === action.kind &&
        page.resourceId === resourceId &&
        (SINGLETONS.has(action.kind) || Boolean(resourceId)),
    )
    if (existing) {
      // 工具入口只消费空白启动页，不关闭包含用户内容的其他标签。
      const pages =
        replaced?.kind === 'new' ? state.pages.filter((page) => page.id !== replaceId) : state.pages
      return { ...state, pages, activeId: existing.id, contentOpen: true, summaryOpen: false }
    }
    if (!replaced && state.pages.length >= MAX_CONTEXT_PAGES) return state
    const page = createPage(replaced?.id || `page-${state.nextId}`, action.kind, resourceId)
    return {
      ...state,
      pages: replaced
        ? state.pages.map((item) => (item.id === page.id ? page : item))
        : [...state.pages, page],
      activeId: page.id,
      nextId: state.nextId + (replaced ? 0 : 1),
      contentOpen: true,
      summaryOpen: false,
    }
  }
  const index = state.pages.findIndex((page) => page.id === action.id)
  if (index < 0) return state
  if (action.type === 'select') return { ...state, activeId: action.id, contentOpen: true }
  if (action.type === 'close') {
    const pages = state.pages.filter((page) => page.id !== action.id)
    return {
      ...state,
      pages,
      activeId:
        state.activeId === action.id
          ? pages[Math.min(index, pages.length - 1)]?.id || ''
          : state.activeId,
      contentOpen: pages.length > 0 && state.contentOpen,
    }
  }
  return {
    ...state,
    pages: state.pages.map((page) =>
      page.id !== action.id
        ? page
        : action.type === 'resource'
          ? { ...page, resourceId: action.resourceId }
          : action.type === 'view'
            ? {
                ...page,
                view: {
                  ...page.view,
                  ...action.patch,
                  reading: { ...page.view.reading, ...action.patch.reading },
                },
              }
            : {
                ...page,
                ...(action.draft !== undefined ? { draft: action.draft } : {}),
                ...(action.url !== undefined ? { url: action.url } : {}),
              },
    ),
  }
}
