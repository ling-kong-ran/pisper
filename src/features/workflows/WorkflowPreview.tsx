import { MarkerType, ReactFlow, type Edge, type Node, type NodeProps } from '@xyflow/react'
import { useMemo } from 'react'
import { cn } from '@/lib/utils'
import type { NodeKind, WorkflowEdge, WorkflowNode } from './types'

type PreviewNodeData = {
  kind: NodeKind
  label: string
  typeLabel: string
}

type PreviewNode = Node<PreviewNodeData, 'workflowPreview'>
type PreviewEdge = Edge<Record<string, never>, 'default'>

function PreviewNodeCard({ data }: NodeProps<PreviewNode>) {
  return (
    <div className={cn('flow-node compact', `type-${data.kind}`)}>
      <small>{data.typeLabel}</small>
      <strong>{data.label}</strong>
    </div>
  )
}

const PREVIEW_NODE_TYPES = { workflowPreview: PreviewNodeCard }

export function WorkflowPreview({
  nodes,
  edges,
  nodeTypeLabel,
}: {
  nodes: WorkflowNode[]
  edges: WorkflowEdge[]
  nodeTypeLabel: (kind: NodeKind) => string
}) {
  const previewNodes = useMemo<PreviewNode[]>(
    () =>
      nodes.map((node) => ({
        id: node.id,
        type: 'workflowPreview',
        position: { x: node.x, y: node.y },
        data: {
          kind: node.kind,
          label: node.label,
          typeLabel: nodeTypeLabel(node.kind),
        },
      })),
    [nodeTypeLabel, nodes],
  )
  const previewEdges = useMemo<PreviewEdge[]>(
    () =>
      edges.map((edge) => ({
        id: edge.id,
        source: edge.source,
        sourceHandle: edge.sourcePort,
        target: edge.target,
        targetHandle: edge.targetPort,
        markerEnd: { type: MarkerType.ArrowClosed, color: 'var(--canvas-edge)' },
        style: { stroke: 'var(--canvas-edge)', strokeWidth: 2 },
      })),
    [edges],
  )

  return (
    <div className="workflow-mini-map">
      <ReactFlow<PreviewNode, PreviewEdge>
        nodes={previewNodes}
        edges={previewEdges}
        nodeTypes={PREVIEW_NODE_TYPES}
        fitView
        fitViewOptions={{ padding: 0.12, minZoom: 0.4, maxZoom: 1.1 }}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={false}
        panOnDrag={false}
        zoomOnScroll={false}
        zoomOnPinch={false}
        zoomOnDoubleClick={false}
        preventScrolling={false}
      />
    </div>
  )
}
