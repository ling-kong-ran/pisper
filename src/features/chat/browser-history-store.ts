// 右侧浏览器的浏览历史（本设备）。唯一读写方；规则在 browser-history.ts。
import { create } from 'zustand'
import { createDebouncedPageStateWriter, readPageState } from '@/lib/debounced-page-state'
import {
  parseBrowserHistory,
  recordBrowserVisit,
  serializeBrowserHistory,
  updateBrowserHistoryTitle,
  type BrowserHistoryEntry,
} from './browser-history'

export const BROWSER_HISTORY_KEY = 'pisper-browser-history'
const WRITE_DELAY_MS = 1000

const appOrigin = () => (typeof window === 'undefined' ? undefined : window.location.origin)

const stored = readPageState(BROWSER_HISTORY_KEY)
const writer = createDebouncedPageStateWriter(BROWSER_HISTORY_KEY, WRITE_DELAY_MS)
writer.noteStored(stored)

const persist = () =>
  writer.schedule(() => serializeBrowserHistory(useBrowserHistoryStore.getState().entries))

export const useBrowserHistoryStore = create<{
  entries: BrowserHistoryEntry[]
  visit: (url: string, title?: string) => void
  title: (url: string, title: string) => void
  clear: () => void
}>((set) => ({
  entries: parseBrowserHistory(stored),
  visit: (url, title = '') => {
    let changed = false
    set((state) => {
      const entries = recordBrowserVisit(state.entries, url, title, Date.now(), appOrigin())
      changed = entries !== state.entries
      return changed ? { entries } : state
    })
    if (changed) persist()
  },
  title: (url, title) => {
    let changed = false
    set((state) => {
      const entries = updateBrowserHistoryTitle(state.entries, url, title, appOrigin())
      changed = entries !== state.entries
      return changed ? { entries } : state
    })
    if (changed) persist()
  },
  clear: () => {
    set({ entries: [] })
    // 用户要求清除：马上写，不等合并窗口。
    persist()
    writer.flush()
  },
}))
