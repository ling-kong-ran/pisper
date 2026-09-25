import { create } from 'zustand'
import {
  createContextPages,
  updateContextPages,
  type ContextPages,
  type ContextPageAction,
  type SessionContextTab,
} from './session-context-pages'
// Ephemeral per-session UI state: never persist browsing URLs or duplicate conversation data.
export const useContextPagesStore = create<{
  sessions: Record<string, ContextPages>
  update: (sessionId: string, initial: SessionContextTab, action: ContextPageAction) => void
}>((set) => ({
  sessions: {},
  update: (sessionId, initial, action) =>
    set((store) => {
      const previous = Object.prototype.hasOwnProperty.call(store.sessions, sessionId)
        ? store.sessions[sessionId]
        : createContextPages(initial)
      const entries = Object.entries(store.sessions)
        .filter(([id]) => id !== sessionId)
        .slice(-31)
      return {
        sessions: Object.fromEntries([
          ...entries,
          [sessionId, updateContextPages(previous, action)],
        ]),
      }
    }),
}))
