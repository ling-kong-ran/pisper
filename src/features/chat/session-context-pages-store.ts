import { create } from 'zustand'
import { createDebouncedPageStateWriter, readPageState } from '@/lib/debounced-page-state'
import {
  browserTabsSnapshot,
  parseBrowserTabs,
  serializeBrowserTabs,
  type PersistedBrowserSession,
} from './browser-persistence'
import {
  createContextPages,
  restoreBrowserPages,
  updateContextPages,
  MAX_CONTEXT_PAGES,
  type ContextPages,
  type ContextPageAction,
  type SessionContextTab,
} from './session-context-pages'

// 任务视图的其余部分（阅读位置、展开项、草稿）只在内存里；只有“预览”里打开的网页落盘，
// 重启后同一会话的预览还是那些网页。关闭分屏不会清理它，只有确认删除任务才 forget（落盘的也一起删）。
export const SESSION_BROWSER_TABS_KEY = 'pisper-session-browser-tabs'
const WRITE_DELAY_MS = 600

const appOrigin = () => (typeof window === 'undefined' ? undefined : window.location.origin)

const storedTabs = readPageState(SESSION_BROWSER_TABS_KEY)
const restored = parseBrowserTabs(storedTabs)
const writer = createDebouncedPageStateWriter(SESSION_BROWSER_TABS_KEY, WRITE_DELAY_MS)
writer.noteStored(storedTabs)
// 每个会话上次保存的页签和时间：页签没变时沿用旧时间，“最近改过的在前”才不会被无关的写入打乱。
const savedSessions = new Map(
  restored.map((session) => [
    session.id,
    {
      signature: JSON.stringify({ active: session.active, tabs: session.tabs }),
      updatedAt: session.updatedAt,
    },
  ]),
)

function serializeSessions(sessions: Record<string, ContextPages>) {
  const now = Date.now()
  const list: PersistedBrowserSession[] = []
  for (const [id, state] of Object.entries(sessions)) {
    const snapshot = browserTabsSnapshot(state, appOrigin())
    if (!snapshot) {
      savedSessions.delete(id)
      continue
    }
    const signature = JSON.stringify(snapshot)
    const previous = savedSessions.get(id)
    const updatedAt = previous?.signature === signature ? previous.updatedAt : now
    savedSessions.set(id, { signature, updatedAt })
    list.push({ id, updatedAt, ...snapshot })
  }
  return serializeBrowserTabs(list)
}

const persist = () =>
  writer.schedule(() => serializeSessions(useContextPagesStore.getState().sessions))

export const useContextPagesStore = create<{
  sessions: Record<string, ContextPages>
  update: (
    sessionId: string,
    initial: SessionContextTab,
    action: ContextPageAction,
  ) => 'updated' | 'limit' | 'unchanged'
  forget: (sessionId: string) => void
}>((set) => ({
  sessions: Object.fromEntries(
    restored.map((session) => [session.id, restoreBrowserPages(session.tabs, session.active)]),
  ),
  update: (sessionId, initial, action) => {
    let outcome: 'updated' | 'limit' | 'unchanged' = 'unchanged'
    let pagesChanged = false
    if (!sessionId) return outcome
    set((store) => {
      const previous = Object.prototype.hasOwnProperty.call(store.sessions, sessionId)
        ? store.sessions[sessionId]
        : createContextPages('id' in action ? initial : undefined)
      const next = updateContextPages(previous, action)
      if (next === previous) {
        if (
          (action.type === 'add' || action.type === 'open') &&
          previous.pages.length >= MAX_CONTEXT_PAGES
        )
          outcome = 'limit'
        return store
      }
      outcome = 'updated'
      // 只显示内容、滚动位置这类工作区变化不碰页签，不用排一次写入。
      pagesChanged =
        next.pages !== previous.pages || next.activeBrowserId !== previous.activeBrowserId
      return { sessions: { ...store.sessions, [sessionId]: next } }
    })
    if (pagesChanged) persist()
    return outcome
  },
  forget: (sessionId) => {
    set((store) => {
      const sessions = { ...store.sessions }
      delete sessions[sessionId]
      return { sessions }
    })
    persist()
  },
}))
