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
