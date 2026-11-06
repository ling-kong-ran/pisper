// 回复里的链接、网页里的新窗口都从这里进检查器的“预览”：已经开着这个地址就切过去，
// 当前页签是空白的就用它，否则开一个新页签。
import { activeBrowserPage } from './session-context-pages'
import { useContextPagesStore } from './session-context-pages-store'

export function openWebPreviewInSessionContext(sessionId: string, url: string) {
  if (!sessionId || !url) return false
  const store = useContextPagesStore.getState()
  const pages = store.sessions[sessionId]
  const current = pages && activeBrowserPage(pages)
  const opened = pages?.pages.find((page) => page.kind === 'browser' && page.url === url)
  if (opened) {
    // 地址没变时检查器看不出有新请求，不会切回“预览”：先收起再打开，才算一次新的打开。
    store.update(sessionId, 'browser', { type: 'workspace', contentOpen: false })
    store.update(sessionId, 'browser', { type: 'select', id: opened.id })
    return true
  }
  const navigate = (id: string) => {
    store.update(sessionId, 'browser', { type: 'select', id })
    store.update(sessionId, 'browser', { type: 'browser', id, url, draft: url })
    return true
  }
  if (current && !current.url) return navigate(current.id)
  // 页签开满了就在当前页签打开，链接不能白点。
  if (store.update(sessionId, 'browser', { type: 'open', kind: 'browser' }) === 'limit')
    return current ? navigate(current.id) : false
  const next = useContextPagesStore.getState().sessions[sessionId]
  const created = next && activeBrowserPage(next)
  return created ? navigate(created.id) : false
}
