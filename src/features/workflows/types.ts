// 工作流领域类型：节点种类（触发器/提示词/技能等）、节点/连线的 JSON 形状
// 与运行时执行的快照结构。
export const WORKFLOW_NODE_KINDS = [
  'trigger',
  'prompt',
  'skill',
  'file',
  'mcp',
  'notification',
  'condition',
  'parallel',
  'approval',
] as const

export type NodeKind = (typeof WORKFLOW_NODE_KINDS)[number]
export type NotificationTarget = 'browser' | 'feishu' | 'weixin'
export type WorkflowInputType = 'string' | 'number' | 'boolean' | 'text'
export type WorkflowExecutionMode = 'read-only' | 'workspace-write' | 'full-access'

export type WorkflowInput = {
  id: string
  name: string
  label: string
  type: WorkflowInputType
  required: boolean
  defaultValue: unknown
  description: string
}

export type WorkflowNode = {
  id: string
  kind: NodeKind
  label: string
  prompt: string
  x: number
  y: number
  model: { provider: string; model: string } | null
  executionMode: WorkflowExecutionMode
  retries: number
  timeoutMinutes: number
  failurePolicy: 'stop' | 'skip'
  enabled: boolean
  outputFormat: 'text' | 'json'
  skillName: string
  requestedToolNames: string[]
  condition: {
    source: string
    operator:
      'exists' | 'not_exists' | 'equals' | 'not_equals' | 'contains' | 'greater_than' | 'less_than'
    value: unknown
  }
  approval: { message: string; timeoutMinutes: number }
  notification: { title: string; content: string }
  notificationTargets: NotificationTarget[]
}

export type WorkflowEdge = {
  id: string
  source: string
  sourcePort: string
  target: string
  targetPort: string
}

export type Workflow = {
  id: string
  name: string
  description: string
  status: 'draft' | 'published'
  revision: number
  cwd: string
  model: { provider: string; model: string } | null
  inputs: WorkflowInput[]
  tags: string[]
  visibility: 'private' | 'shared'
  notifications: NotificationTarget[]
  nodes: WorkflowNode[]
  edges: WorkflowEdge[]
  createdAt?: string
  updatedAt?: string
  publishedAt?: string | null
  lastRunAt?: string | null
  lastStatus?: string
  lastSummary?: string
  lastError?: string
}

export type WorkflowRunNode = {
  id: string
  label: string
  kind: NodeKind
  status: string
  attempts: number
  summary: string
  output: unknown
  error: string
  sessionId: string
  startedAt?: string | null
  finishedAt?: string | null
  durationMs?: number
  selectedPort?: string
  skipReason?: string
  approval?: {
    message?: string
    requestedAt?: string
    expiresAt?: string
    approved?: boolean
    comment?: string
    resolvedAt?: string
  } | null
}

export type WorkflowRun = {
  id: string
  workflowId: string
  workflowName?: string
  workflowRevision?: number
  trigger?: string
  sourceSessionId?: string
  sourceMessage?: string
  retryOf?: string
  inputs?: Record<string, unknown>
  status: 'running' | 'waiting_approval' | 'completed' | 'failed' | 'cancelled' | 'interrupted'
  startedAt: string
  finishedAt?: string | null
  durationMs?: number
  completedNodes?: number
  totalNodes?: number
  currentNodeId?: string
  currentNodeLabel?: string
  summary?: string
  error?: string
  assets?: Array<{ id: string; name?: string; path?: string; mimeType?: string }>
  nodes?: WorkflowRunNode[]
}

export type WorkflowModel = { provider: string; model: string; label: string }
export type NotificationTargets = Record<NotificationTarget, { enabled: boolean }>

export type WorkflowsData = {
  workflows: Workflow[]
  runs: WorkflowRun[]
  limits: { maxConcurrent: number; running: number }
  notificationTargets: NotificationTargets
  models: WorkflowModel[]
  skills: Array<{ id: string; name: string; description?: string }>
  cwd: string
}

export type WorkflowMutationResult = { workflow: Workflow; state: WorkflowsData }
