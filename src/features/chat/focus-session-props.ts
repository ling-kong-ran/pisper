// 聚焦会话视图的组件 props 类型：从 FocusSession.tsx 拆出以控制单文件体积。
// FocusSession.tsx 通过 re-export 保持对外导出签名不变。
import type { ReactNode } from 'react'
import type { Notify } from '@/app/route-context'
import type {
  ChatAttachment,
  ChatMessage,
  EntityRecord,
  ModelOption,
  Plan,
  ResourceInvocation,
  SessionSummary,
  SessionWorkspaceActivity,
} from '@/types/chat'
import type { TranscriptLoadState } from '@/features/chat/FocusTranscript'
import type {
  BranchFromHereOptions,
  CompactionPreferencePatch,
  WithdrawnInput,
} from '@/features/chat/chat-api'
import type { NewTaskDraftChrome } from '@/features/chat/session-workspace-context'

export type FocusSessionProps = {
  session: SessionSummary
  // 新任务草稿（还没有会话）：页头写“新任务”，转录区是问候语，输入框多出项目芯片与 + 菜单里的排队选项。
  draft?: NewTaskDraftChrome
  // 没有消息时转录区的说明（例如排队中的新任务）；不给时用默认的空状态。
  emptyState?: ReactNode
  shortcutEnabled?: boolean
  messages: ChatMessage[]
  transcriptLoadState?: TranscriptLoadState
  messageStart?: number | null
  hasNewer?: boolean
  hasOlder?: boolean
  loadingOlder?: boolean
  olderError?: string
  model: string
  thinkingLevel?: string
  availableThinkingLevels?: string[]
  thinkingStatus?: string
  thinkingMessage?: string
  executionMode: string
  runMode?: string | null
  goal?: EntityRecord | null
  team?: EntityRecord | null
  plan?: Plan | null
  currentActivity?: EntityRecord | null
  activityFeed: EntityRecord[]
  tools: EntityRecord[]
  thinkingText?: string
  queuedInputs: EntityRecord[]
  // 同一文件夹里的协作：最近在这里改文件的其他会话、等着处理的留言。
  workspace?: SessionWorkspaceActivity | null
  withdrawingInputIds?: string[]
  compaction?: EntityRecord | null
  contextUsage?: EntityRecord | null
  sessionUsage?: EntityRecord | null
  sessionTreeRevision?: number
  sessionTreePulse?: number
  cwd?: string
  availableModels: ModelOption[]
  pendingRuntimeSelection?: { model?: string; thinkingLevel?: string }
  switchingModel?: boolean
  switchingThinking?: boolean
  switchingCwd?: boolean
  switchingPermission?: boolean
  streaming?: boolean
  runStartedAt?: string | null
  lastActivityAt?: string | null
  runFinishedAt?: string | null
  runStopped?: boolean
  runNotice?: string
  approvals: EntityRecord[]
  error?: string
  pendingAsset?: ChatAttachment | null
  notify?: Notify
  onOpenModelSettings?: () => void
  onAssetConsumed?: () => void
  onLoadLatest?: () => Promise<void> | void
  onLoadOlder?: () => Promise<boolean> | boolean
  onModelChange: (model: string) => Promise<void> | void
  onThinkingLevelChange: (level: string) => Promise<void> | void
  onExecutionModeChange: (mode: string) => Promise<boolean> | boolean
  onRunModeChange: (mode: string) => Promise<boolean> | boolean
  onGoalPause?: () => Promise<void> | void
  onGoalBudgetChange?: (tokenBudget: number | null) => Promise<void> | void
  onCompact?: () => Promise<void> | void
  onCompactionPreferenceChange?: (patch: CompactionPreferencePatch) => Promise<void> | void
  onApproval: (approvalId: string, approved: boolean) => Promise<void> | void
  // 代理的提问（ask_user，A5）：Runtime 的待回答问题原样传入，由问题卡解析。
  questions?: EntityRecord[]
  onAnswerQuestion?: (
    questionId: string,
    body: { answers: { selected: string[]; other?: string }[] } | { skip: true },
  ) => Promise<void>
  onWorkspace: () => void
  onRename: () => void
  onBranchFromHere: (
    boundaryEntryId: string,
    options?: BranchFromHereOptions,
  ) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
  onRetryLastTurn: () => Promise<void> | void
  onTreeNavigated?: () => Promise<void> | void
  onSend: (
    value: string,
    attachments: ChatAttachment[],
    goalMode: boolean,
    teamMode: boolean,
    goalTokenBudget: number | null,
    invocation?: ResourceInvocation | null,
    onAccepted?: () => void,
  ) => Promise<boolean | void> | boolean | void
  onQueue?: (
    value: string,
    attachments: ChatAttachment[],
    behavior: string,
    onAccepted?: () => void,
  ) => Promise<boolean> | boolean
  onWithdrawQueuedInput?: (inputId: string) => Promise<WithdrawnInput | null>
  onSendQueuedInputNow?: (inputId: string) => Promise<boolean>
  onDeliverSessionMessages?: () => Promise<void>
  onDismissSessionMessages?: () => Promise<void>
  onAbort: () => Promise<void> | void
}
