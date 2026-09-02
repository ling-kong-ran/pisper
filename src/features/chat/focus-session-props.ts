// 聚焦会话视图的组件 props 类型：从 FocusSession.tsx 拆出以控制单文件体积。
// FocusSession.tsx 通过 re-export 保持对外导出签名不变。
import type { Notify } from '@/app/route-context'
import type {
  ChatAttachment,
  ChatMessage,
  EntityRecord,
  ModelOption,
  Plan,
  ResourceInvocation,
  SessionSummary,
} from '@/types/chat'
import type { TranscriptLoadState } from '@/features/chat/FocusTranscript'

export type FocusSessionProps = {
  session: SessionSummary
  messages: ChatMessage[]
  transcriptLoadState?: TranscriptLoadState
  messageStart?: number | null
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
  compaction?: EntityRecord | null
  contextUsage?: EntityRecord | null
  sessionUsage?: EntityRecord | null
  sessionTreeRevision?: number
  sessionTreePulse?: number
  cwd?: string
  availableModels: ModelOption[]
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
  canSplit?: boolean
  // 是否提供「关闭面板」入口：移动端单会话视图没有可关闭的面板。
  canClosePanel?: boolean
  notify?: Notify
  onOpenModelSettings?: () => void
  onAssetConsumed?: () => void
  onLoadOlder?: () => Promise<boolean> | boolean
  onModelChange: (model: string) => Promise<void> | void
  onThinkingLevelChange: (level: string) => Promise<void> | void
  onExecutionModeChange: (mode: string) => Promise<boolean> | boolean
  onRunModeChange: (mode: string) => Promise<boolean> | boolean
  onGoalPause?: () => Promise<void> | void
  onGoalBudgetChange?: (tokenBudget: number | null) => Promise<void> | void
  onCompact?: () => Promise<void> | void
  onCompactionThresholdChange?: (thresholdPercent: number) => Promise<void> | void
  onApproval: (approvalId: string, approved: boolean) => Promise<void> | void
  onWorkspace: () => void
  onRename: () => void
  onBranchFromHere: (boundaryEntryId: string) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
  onTreeNavigated?: () => Promise<void> | void
  onSplitLeft: () => void
  onSplitRight: () => void
  onSplitTop: () => void
  onSplitBottom: () => void
  onClosePanel: () => void
  onSend: (
    value: string,
    attachments: ChatAttachment[],
    goalMode: boolean,
    teamMode: boolean,
    goalTokenBudget: number | null,
    invocation?: ResourceInvocation | null,
  ) => Promise<void> | void
  onQueue?: (
    value: string,
    attachments: ChatAttachment[],
    behavior: string,
  ) => Promise<boolean> | boolean
  onAbort: () => Promise<void> | void
}
