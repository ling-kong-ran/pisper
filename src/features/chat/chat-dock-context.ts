// 会话打开请求 / Dock 布局事件的处理封装，供各组件复用。
import { createContext } from 'react'
import type { Notify } from '@/app/route-context'
import type {
  ChatAttachment,
  ModelOption,
  PendingAsset,
  ResourceInvocation,
  SessionState,
  SessionSummary,
} from '@/types/chat'
import type { SessionOpenDisposition } from './dock-layout'

export type ChatDockContextValue = {
  sessions: SessionSummary[]
  // 会话状态走面板级订阅（流式高频），不再随 context 广播：
  // 订阅只在该会话 state 引用变化时触发，快照引用稳定。
  subscribeSessionState: (id: string, listener: () => void) => () => void
  getSessionState: (id: string) => SessionState
  defaultModel: string
  availableModels: ModelOption[]
  globalError: string
  activeId: string
  compactDock: boolean
  sessionTreePulseSessionId: string
  sessionTreePulseToken: number
  pendingAsset: PendingAsset | null
  onAssetConsumed: () => void
  // 通用通知与「打开模型设置」入口（视觉生成引导等场景使用）。
  notify: Notify
  openModelSettings: () => void
  loadSessionMessages: (id: string, options?: { force?: boolean; limit?: number }) => Promise<void>
  loadOlderMessages: (id: string) => Promise<boolean>
  sendPrompt: (
    value: string,
    sessionId: string,
    attachments?: ChatAttachment[],
    goalMode?: boolean,
    teamMode?: boolean,
    goalTokenBudget?: number | null,
    invocation?: ResourceInvocation | null,
  ) => Promise<void>
  queuePrompt: (
    value: string,
    sessionId: string,
    attachments?: ChatAttachment[],
    behavior?: string,
  ) => Promise<boolean>
  abort: (sessionId: string) => Promise<void>
  pauseGoal: (sessionId: string) => Promise<void>
  setGoalBudget: (sessionId: string, tokenBudget: number | null) => Promise<void>
  compactSession: (sessionId: string) => Promise<void>
  setCompactionThreshold: (thresholdPercent: number) => Promise<void>
  switchSessionModel: (sessionId: string, model: string) => Promise<void>
  loadSessionThinkingLevel: (sessionId: string) => Promise<void>
  switchSessionThinkingLevel: (sessionId: string, level: string) => Promise<void>
  switchSessionExecutionMode: (sessionId: string, mode: string) => Promise<boolean>
  switchSessionRunMode: (sessionId: string, mode: string) => Promise<boolean>
  resolveToolApproval: (sessionId: string, approvalId: string, approved: boolean) => Promise<void>
  selectSessionWorkspace: (session: SessionSummary) => Promise<void>
  renameSession: (session: SessionSummary) => Promise<void>
  branchFromEntry: (session: SessionSummary, boundaryEntryId: string) => Promise<void>
  createChildSession: (session: SessionSummary, boundaryEntryId: string) => Promise<void>
  reloadSessionBranch: (sessionId: string) => Promise<void>
  splitDockPanel: (panelId: string, direction: Exclude<SessionOpenDisposition, 'open'>) => void
  closeDockPanel: (panelId: string) => void
}

export const ChatDockContext = createContext<ChatDockContextValue | null>(null)
