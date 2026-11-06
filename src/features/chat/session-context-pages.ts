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
  // 网页自己的标题（只有原生浏览器拿得到），标签上优先显示它。
  title: string
  // Agent 正在用这个页签。离开“预览”时 WebView 不关，只隐藏：操作还要继续。
  agent: boolean
  view: ContextPageView
}
export type ContextPages = {
  pages: ContextPage[]
  activeId: string
  // 检查器“预览”里当前显示的浏览器页签。和 activeId 分开：切换页签不该改“只显示内容”这类全局状态。
  activeBrowserId: string
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
  | { type: 'browser'; id: string; draft?: string; url?: string; title?: string; agent?: boolean }
  | { type: 'browser-tab'; id: string }
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
    title: '',
    agent: false,
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
    activeBrowserId: kind === 'browser' ? 'page-1' : '',
    nextId: kind ? 2 : 1,
    contentOpen: false,
    summaryOpen: false,
    contentWidth: null,
    conversationScrollTop: null,
  }
}

export function activeBrowserPage(state: ContextPages) {
  const browsers = state.pages.filter((page) => page.kind === 'browser')
  return browsers.find((page) => page.id === state.activeBrowserId) ?? browsers[0]
}

// 关掉当前页签后选它右边的，没有再选左边的，和浏览器一样。
function neighborBrowserId(pages: ContextPage[], closingId: string) {
  const browsers = pages.filter((page) => page.kind === 'browser')
  const index = browsers.findIndex((page) => page.id === closingId)
  return (browsers[index + 1] ?? browsers[index - 1])?.id || ''
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
      return {
        ...state,
        pages,
        activeId: existing.id,
        activeBrowserId: existing.kind === 'browser' ? existing.id : state.activeBrowserId,
        contentOpen: true,
        summaryOpen: false,
      }
    }
    if (!replaced && state.pages.length >= MAX_CONTEXT_PAGES) return state
    const page = createPage(replaced?.id || `page-${state.nextId}`, action.kind, resourceId)
    return {
      ...state,
      pages: replaced
        ? state.pages.map((item) => (item.id === page.id ? page : item))
        : [...state.pages, page],
      activeId: page.id,
      activeBrowserId: action.kind === 'browser' ? page.id : state.activeBrowserId,
      nextId: state.nextId + (replaced ? 0 : 1),
      contentOpen: true,
      summaryOpen: false,
    }
  }
  const index = state.pages.findIndex((page) => page.id === action.id)
  if (index < 0) return state
  if (action.type === 'select')
    return {
      ...state,
      activeId: action.id,
      activeBrowserId: state.pages[index].kind === 'browser' ? action.id : state.activeBrowserId,
      contentOpen: true,
    }
  if (action.type === 'browser-tab') {
    return state.pages[index].kind === 'browser' ? { ...state, activeBrowserId: action.id } : state
  }
  if (action.type === 'close') {
    const pages = state.pages.filter((page) => page.id !== action.id)
    const nextBrowserId =
      state.activeBrowserId === action.id
        ? neighborBrowserId(state.pages, action.id)
        : state.activeBrowserId
    return {
      ...state,
      pages,
      // 关浏览器页签时留在浏览器页签上，不让检查器跟着 activeId 跳到别的标签。
      activeId:
        state.activeId === action.id
          ? (state.pages[index].kind === 'browser' && nextBrowserId) ||
            pages[Math.min(index, pages.length - 1)]?.id ||
            ''
          : state.activeId,
      activeBrowserId: nextBrowserId,
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
                // 换了地址旧标题就不对了，新页面的标题到了再显示。
                ...(action.url !== undefined && action.url !== page.url ? { title: '' } : {}),
                ...(action.title !== undefined ? { title: action.title } : {}),
                ...(action.agent !== undefined ? { agent: action.agent } : {}),
              },
    ),
  }
}
