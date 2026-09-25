export type SessionContextTab = 'files' | 'plan' | 'browser'
export type ContextPage = { id: string; kind: SessionContextTab; draft: string; url: string }
export type ContextPages = { pages: ContextPage[]; activeId: string; nextId: number }
export type ContextPageAction =
  | { type: 'add'; kind: SessionContextTab }
  | { type: 'select'; id: string }
  | { type: 'close'; id: string }
  | { type: 'kind'; id: string; kind: SessionContextTab }
  | { type: 'browser'; id: string; draft?: string; url?: string }
export const MAX_CONTEXT_PAGES = 12
export function createContextPages(kind: SessionContextTab = 'files'): ContextPages {
  return { pages: [{ id: 'page-1', kind, draft: '', url: '' }], activeId: 'page-1', nextId: 2 }
}
export function updateContextPages(state: ContextPages, action: ContextPageAction): ContextPages {
  if (action.type === 'add') {
    if (state.pages.length >= MAX_CONTEXT_PAGES) return state
    const page = { id: `page-${state.nextId}`, kind: action.kind, draft: '', url: '' }
    return { pages: [...state.pages, page], activeId: page.id, nextId: state.nextId + 1 }
  }
  const index = state.pages.findIndex((page) => page.id === action.id)
  if (index < 0) return state
  if (action.type === 'select') return { ...state, activeId: action.id }
  if (action.type === 'close') {
    const pages = state.pages.filter((page) => page.id !== action.id)
    // Keep a default page for the next reopen; the UI closes the panel on the last tab.
    if (!pages.length) return createContextPages()
    return {
      ...state,
      pages,
      activeId:
        state.activeId === action.id ? pages[Math.min(index, pages.length - 1)].id : state.activeId,
    }
  }
  return {
    ...state,
    pages: state.pages.map((page) =>
      page.id !== action.id
        ? page
        : action.type === 'kind'
          ? { ...page, kind: action.kind }
          : {
              ...page,
              ...(action.draft !== undefined ? { draft: action.draft } : {}),
              ...(action.url !== undefined ? { url: action.url } : {}),
            },
    ),
  }
}
