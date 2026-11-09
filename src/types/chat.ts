// 聊天领域类型：消息/工具/计划/会话摘要/会话状态。
// EntityRecord 是逃生口——流式协议携带前端预先未知的扩展字段，
// 在运行时协议提供判别式事件 schema 前统一集中在这里。
import type { BoardCard } from '@shared/session-board-status.mjs'

export type EntityRecord = Record<string, any>

export type ChatAttachment = EntityRecord & {
  id?: string
  kind?: string
  name?: string
  mimeType?: string
  size?: number
  data?: string
  path?: string
}

export type ModelOption = {
  key: string
  provider: string
  modelId: string
  label: string
  providerName: string
}

export type PendingAsset = {
  asset: ChatAttachment
  targetSessionId?: string
}

export type ResourceInvocation = {
  kind: 'skill' | 'workflow' | 'tool'
  resourceId: string
  resourceName: string
  arguments?: Record<string, unknown>
  behavior?: 'foreground' | 'background'
  runId?: string
}

export type ChatMessage = EntityRecord & {
  id: string
  role: string
  text?: string
  streaming?: boolean
  attachments?: ChatAttachment[]
  turnBoundaryEntryId?: string
  // 运行中追加的用户消息：Runtime 给的输入 id；还没送达代理时 queuedPending 为真（排在正在写的回复后面）。
  queuedInputId?: string
  queuedPending?: boolean
  // 被“立即发送”打断的那段回复（定格在原位，下面标“已打断”）。
  interrupted?: boolean
  // 别的 Bot 说进 Bot 对话的话：发话 Bot 的 id（气泡上面显示它的头像与名字）。
  fromBot?: string
  // 同一文件夹里别的会话留的言：发话会话（气泡上面写“来自会话「X」”）。
  fromSessions?: SessionMessageSender[]
}

export type SessionMessageSender = { id: string; title: string }

// 同一文件夹里的协作（Runtime /live 的 workspace 字段）：最近在这里改文件的其他会话、等着处理的留言。
export type SessionWorkspaceActivity = {
  neighbours: {
    id: string
    title: string
    running: boolean
    fileCount: number
    files: string[]
    lastChangedAt: string
  }[]
  messages: { id: string; from: SessionMessageSender; text: string; at: string }[]
}

export type ToolActivity = EntityRecord & {
  id?: string
  type?: string
  name?: string
  status?: string
  message?: string
}

export type PlanItem = EntityRecord & {
  id?: string
  title?: string
  status?: string
  note?: string
  assignee?: string
  dependsOn?: string[]
}

export type Plan = EntityRecord & {
  items: PlanItem[]
}

export type TeamTask = {
  id?: string
  taskName?: string
  role?: string
  status?: string
  error?: string
  output?: string
}

export type TeamCommunication = {
  id: string
  fromTaskName?: string
  fromAgentId?: string
  toTaskName?: string
  toAgentId?: string
  message?: string
}

export type TeamState = {
  id?: string
  sessionId?: string
  goalId?: string
  objective?: string
  status?: string
  tokenBudget?: number
  tokenUsed?: number
  tokensUsed?: number
  teamTokenBudget?: number
  tasks?: TeamTask[]
  completedTaskCount?: number
  taskCount?: number
  blockers?: string[]
  conflicts?: string[]
  scriptPath?: string
  communications?: TeamCommunication[]
  summary?: { text?: string }
}

export type SessionSummary = EntityRecord & {
  id: string
  name?: string
  modified?: string
  pinned?: boolean
  archived?: boolean
  unread?: boolean
  needsAttention?: boolean
  attentionReason?: 'approval' | 'failure' | null
  lastCompletedAt?: string | null
  firstMessage?: string
  // 最后一条有文字的用户 / 助手消息（已压成一行并截短）；Runtime 还没读到时没有这个字段。
  lastMessage?: { role: 'user' | 'assistant'; text: string }
  model?: string
  thinkingLevel?: string
  cwd?: string
  // 不选项目新建的会话：cwd 是 Runtime 为它新建的空文件夹，界面上归入“无项目”。
  projectless?: boolean
  // Bot 的长期会话：Bot 的目录（Bot 在执行节点上时会话 cwd 只是占位目录），归组与筛选按它。
  projectCwd?: string
  // 落在远程机器上的会话：cwd 只是这边的占位目录，工具在这台机器的这个目录里执行。
  machine?: { id: string; kind: 'ssh' | 'node'; name: string; cwd: string }
  // Bot 的长期会话：是哪个 Bot（Bots 实验功能打开时由 Runtime 带上）。会话页头据此换成 Bot 的头像与设置入口。
  botId?: string
  streaming?: boolean
  plan?: Plan | null
  team?: TeamState | null
  lineage?: {
    parentSessionId?: string
    sourceEntryId?: string
    sourceSessionName?: string
    derivedAt?: string | null
    childSessionIds?: string[]
  } | null
  // 只出现在第 2 期 Runtime 的 view=board 与看板事件流条目里；默认 GET /api/sessions 不带。
  // 看板 store 把它拆进实时覆盖层，权威目录缓存里的会话行不保存 card。
  card?: BoardCard
}

// 历史阅读是有界客户端投影；messages 始终保留实时流可继续更新的最新身份。
export type SessionMessageWindow = {
  messages: ChatMessage[]
  messageStart: number
  hasOlder: boolean
  olderCursor: string | null
  hasNewer?: boolean
}

export type SessionState = EntityRecord & {
  historyWindow?: SessionMessageWindow | null
  runtimeSelectionRevision?: number
  pendingRuntimeSelection?: { model?: string; thinkingLevel?: string }
  messages: ChatMessage[]
  tools: ToolActivity[]
  approvals: EntityRecord[]
  // 代理的提问（ask_user，A5）：按原样保存 Runtime 的待回答问题，界面用 session-questions 解析。
  questions?: EntityRecord[]
  agents: EntityRecord[]
  currentActivity: EntityRecord | null
  activityFeed: EntityRecord[]
  lifecycle: EntityRecord | null
  sessionTreeRevision: number
  thinkingText: string
  queuedInputs: EntityRecord[]
  workspace?: SessionWorkspaceActivity | null
  // 这一轮里已经送达代理的排队输入 id：HTTP 响应晚于送达时，乐观气泡直接按“已送达”放进时间线。
  deliveredInputIds?: string[]
  hadQueuedInput: boolean
  plan: Plan | null
  team: TeamState | null
  executionMode: string | null
  runMode: string | null
  contextUsage: EntityRecord | null
  sessionUsage: EntityRecord | null
  promptCache: EntityRecord | null
  compaction: EntityRecord | null
  streaming: boolean
  error: string
  loaded: boolean
  messageStart: number | null
  hasOlder: boolean
  olderCursor: string | null
}
