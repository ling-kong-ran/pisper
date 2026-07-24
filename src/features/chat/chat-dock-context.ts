import { createContext } from 'react'
import type {
  ChatAttachment,
  ModelOption,
  PendingAsset,
  SandboxStatus,
  SessionState,
  SessionSummary,
} from '../../types/chat'
import type { SessionOpenDisposition } from './dock-layout'

export type ChatDockContextValue = {
  sessions: SessionSummary[]
  sessionStates: Record<string, SessionState>
  defaultModel: string
  availableModels: ModelOption[]
  globalError: string
  activeId: string
  compactDock: boolean
  pendingAsset: PendingAsset | null
  onAssetConsumed: () => void
  loadSessionMessages: (id: string, options?: { force?: boolean; limit?: number }) => Promise<void>
  loadOlderMessages: (id: string) => Promise<boolean>
  sendPrompt: (
    value: string,
    sessionId: string,
    attachments?: ChatAttachment[],
    goalMode?: boolean,
  ) => Promise<void>
  queuePrompt: (value: string, sessionId: string, behavior?: string) => Promise<boolean>
  abort: (sessionId: string) => Promise<void>
  pauseGoal: (sessionId: string) => Promise<void>
  switchSessionModel: (sessionId: string, model: string) => Promise<void>
  switchSessionExecutionMode: (sessionId: string, mode: string) => Promise<boolean>
  sandboxStatus: SandboxStatus
  resolveToolApproval: (sessionId: string, approvalId: string, approved: boolean) => Promise<void>
  setWorkspaceSession: (session: SessionSummary) => void
  renameSession: (session: SessionSummary) => Promise<void>
  splitDockPanel: (panelId: string, direction: Exclude<SessionOpenDisposition, 'open'>) => void
  closeDockPanel: (panelId: string) => void
  openRail: (() => void) | null
}

export const ChatDockContext = createContext<ChatDockContextValue | null>(null)
