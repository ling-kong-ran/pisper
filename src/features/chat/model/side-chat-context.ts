import { createContext, useContext } from 'react'
import type { SessionState } from '@/types/chat'
import type { SideChatResponse } from '@/features/chat/model/side-chat-api'

export type SideChatRuntime = {
  subscribeSessionState: (id: string, listener: () => void) => () => void
  getSessionState: (id: string) => SessionState
  loadSessionMessages: (id: string, options?: { force?: boolean }) => Promise<void>
  loadOlderMessages: (id: string) => Promise<boolean>
  syncLiveSession: (id: string) => Promise<void>
  retainSessionState: (id: string) => () => void
  discard: (id: string) => void
  send: (id: string, text: string) => Promise<void>
  retry: (id: string) => Promise<void>
  abort: (id: string) => Promise<void>
  approve: (id: string, approvalId: string, approved: boolean) => Promise<void>
}

export type SideChatEntry = SideChatResponse & {
  loading: boolean
  error: string
  draft: string
  expired: boolean
}
export const EMPTY_ENTRY: SideChatEntry = {
  session: null,
  expiresAt: null,
  created: false,
  loading: false,
  error: '',
  draft: '',
  expired: false,
}

type SideChatContextValue = {
  entries: Record<string, SideChatEntry>
  runtime: SideChatRuntime
  load: (parentId: string, create?: boolean) => Promise<SideChatResponse | null>
  setDraft: (parentId: string, draft: string) => void
}
export const SideChatContext = createContext<SideChatContextValue | null>(null)

export function useSideChat(parentId: string) {
  const context = useContext(SideChatContext)
  if (!context) throw new Error('Side chat requires its runtime provider')
  return { ...context, entry: context.entries[parentId] ?? EMPTY_ENTRY }
}
