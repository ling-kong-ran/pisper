export type WorkflowGraphNode = {
  id: string
  kind?: string
  enabled?: boolean
  x: number
  y: number
}

export type WorkflowGraphEdge = {
  id: string
  source: string
  sourcePort: string
  target: string
  targetPort: string
}

export function createLinearWorkflowEdges<T extends WorkflowGraphNode>(
  nodes: T[],
  idFactory?: (index: number, sourceId: string, targetId: string) => string,
): WorkflowGraphEdge[]

export function wouldCreateWorkflowCycle<T extends WorkflowGraphNode>(
  nodes: T[],
  edges: WorkflowGraphEdge[],
  source: string,
  target: string,
  sourcePort?: string,
): boolean
