// 单会话布局（会话页）没有 Dockview 右侧预览面板：外链预览改为打开该会话检查器里的“网页预览”页。
// 已有预览页时复用并切到新地址，与旧 Dock 预览面板“更新地址并聚焦”的行为一致。
import { useContextPagesStore } from './session-context-pages-store'

export function openWebPreviewInSessionContext(sessionId: string, url: string) {
  if (!sessionId || !url) return false
  const store = useContextPagesStore.getState()
  const existing = store.sessions[sessionId]?.pages.find((page) => page.kind === 'browser')
  if (existing) {
    store.update(sessionId, 'browser', { type: 'select', id: existing.id })
    store.update(sessionId, 'browser', { type: 'browser', id: existing.id, url, draft: url })
    return true
  }
  if (store.update(sessionId, 'browser', { type: 'open', kind: 'browser' }) === 'limit')
    return false
  const pages = useContextPagesStore.getState().sessions[sessionId]
  const opened = pages?.pages.find((page) => page.id === pages.activeId)
  if (opened?.kind !== 'browser') return false
  store.update(sessionId, 'browser', { type: 'browser', id: opened.id, url, draft: url })
  return true
}
