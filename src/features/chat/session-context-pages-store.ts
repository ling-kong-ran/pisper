import { create } from 'zustand'
import {
  createContextPages,
  updateContextPages,
  MAX_CONTEXT_PAGES,
  type ContextPages,
  type ContextPageAction,
  type SessionContextTab,
} from './session-context-pages'

// 临时任务视图不落盘；关闭分屏不会清理它，只有确认删除任务才 forget。
export const useContextPagesStore = create<{
  sessions: Record<string, ContextPages>
  update: (
    sessionId: string,
    initial: SessionContextTab,
    action: ContextPageAction,
  ) => 'updated' | 'limit' | 'unchanged'
  forget: (sessionId: string) => void
}>((set) => ({
  sessions: {},
  update: (sessionId, initial, action) => {
    let outcome: 'updated' | 'limit' | 'unchanged' = 'unchanged'
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
      return { sessions: { ...store.sessions, [sessionId]: next } }
    })
    return outcome
  },
  forget: (sessionId) =>
    set((store) => {
      const sessions = { ...store.sessions }
      delete sessions[sessionId]
      return { sessions }
    }),
}))
