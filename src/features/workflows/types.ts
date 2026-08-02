export const WORKFLOW_NODE_KINDS = [
  'trigger',
  'prompt',
  'file',
  'mcp',
  'notification',
  'condition',
  'parallel',
  'approval',
] as const

export type NodeKind = (typeof WORKFLOW_NODE_KINDS)[number]
export type NotificationTarget = 'browser' | 'feishu' | 'weixin'

export type WorkflowNode = {
  id: string
  kind: NodeKind
  label: string
  prompt: string
  x: number
  y: number
  model: { provider: string; model: string } | null
  retries: number
  timeoutMinutes: number
  failurePolicy: 'stop' | 'skip'
  enabled: boolean
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
  cwd: string
  model: { provider: string; model: string } | null
  notifications: NotificationTarget[]
  nodes: WorkflowNode[]
  edges: WorkflowEdge[]
  lastRunAt?: string | null
  lastStatus?: string
}

export type WorkflowRun = {
  id: string
  workflowId: string
  status: 'running' | 'completed' | 'failed' | 'cancelled'
  startedAt: string
  durationMs?: number
  completedNodes?: number
  totalNodes?: number
  currentNodeLabel?: string
  summary?: string
  error?: string
}

export type WorkflowModel = { provider: string; model: string; label: string }
export type NotificationTargets = Record<NotificationTarget, { enabled: boolean }>

export type WorkflowsData = {
  workflows: Workflow[]
  runs: WorkflowRun[]
  limits: { maxConcurrent: number; running: number }
  notificationTargets: NotificationTargets
  models: WorkflowModel[]
  cwd: string
}

export type WorkflowMutationResult = { workflow: Workflow; state: WorkflowsData }
