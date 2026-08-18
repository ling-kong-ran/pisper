// 聊天领域类型：消息/工具/计划/会话摘要/会话状态。
// EntityRecord 是逃生口——流式协议携带前端预先未知的扩展字段，
// 在运行时协议提供判别式事件 schema 前统一集中在这里。
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

export type SessionSummary = EntityRecord & {
  id: string
  name?: string
  modified?: string
  firstMessage?: string
  model?: string
  thinkingLevel?: string
  cwd?: string
  streaming?: boolean
  plan?: Plan | null
  lineage?: {
    parentSessionId?: string
    sourceEntryId?: string
    sourceSessionName?: string
    derivedAt?: string | null
    childSessionIds?: string[]
  } | null
}

export type SessionState = EntityRecord & {
  messages: ChatMessage[]
  tools: ToolActivity[]
  approvals: EntityRecord[]
  agents: EntityRecord[]
  currentActivity: EntityRecord | null
  activityFeed: EntityRecord[]
  lifecycle: EntityRecord | null
  sessionTreeRevision: number
  thinkingText: string
  queuedInputs: EntityRecord[]
  hadQueuedInput: boolean
  plan: Plan | null
  executionMode: string | null
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
