// Pisper's streaming protocol carries tool-specific extension fields that are not
// known to the frontend ahead of time. Keep the escape hatch centralized here
// until the server protocol exposes discriminated event schemas.
export type EntityRecord = Record<string, any>

export type ChatAttachment = EntityRecord & {
  id?: string
  kind?: string
  name?: string
  mimeType?: string
  size?: number
  data?: string
}

export type ModelOption = {
  key: string
  provider: string
  modelId: string
  label: string
  providerName: string
}

export type SandboxStatus = EntityRecord & {
  state: string
  supported?: boolean
  platform?: string
  message?: string
}

export type PendingAsset = {
  asset: ChatAttachment
  targetSessionId?: string
}

export type ChatMessage = EntityRecord & {
  id: string
  role: string
  text?: string
  streaming?: boolean
  attachments?: ChatAttachment[]
}

export type ToolActivity = EntityRecord & {
  id?: string
  type?: string
  name?: string
  status?: string
  message?: string
}

export type TaskListItem = EntityRecord & {
  id?: string
  title?: string
  status?: string
  note?: string
  assignee?: string
  dependsOn?: string[]
}

export type TaskList = EntityRecord & {
  items: TaskListItem[]
}

export type SessionSummary = EntityRecord & {
  id: string
  name?: string
  modified?: string
  firstMessage?: string
  model?: string
  cwd?: string
  streaming?: boolean
  taskList?: TaskList | null
}

export type SessionState = EntityRecord & {
  messages: ChatMessage[]
  tools: ToolActivity[]
  approvals: EntityRecord[]
  agents: EntityRecord[]
  currentActivity: EntityRecord | null
  activityFeed: EntityRecord[]
  thinkingText: string
  queuedInputs: EntityRecord[]
  hadQueuedInput: boolean
  taskList: TaskList | null
  executionMode: string | null
  contextUsage: EntityRecord | null
  compaction: EntityRecord | null
  streaming: boolean
  error: string
  loaded: boolean
  messageStart: number | null
  hasOlder: boolean
  olderCursor: string | null
}
