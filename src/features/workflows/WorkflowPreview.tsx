// 工作流预览：只读渲染工作流图为静态预览（无编辑交互）。
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
    <div
      // 只读预览沿用画布节点的细线卡片语言，尺寸缩小、不可拖动。
      className={cn(
        'flow-node compact flex w-[100px] min-h-[42px] cursor-default flex-col justify-center gap-0.5 rounded-sm border border-stroke bg-panel px-2 py-1.5 text-left [&_small]:text-xs [&_small]:text-content-muted [&_strong]:truncate [&_strong]:text-xs [&_strong]:font-medium',
        `type-${data.kind}`,
      )}
    >
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
        style: { stroke: 'var(--canvas-edge)', strokeWidth: 1.5 },
      })),
    [edges],
  )

  return (
    <div className="workflow-mini-map relative mt-1 h-[150px] overflow-hidden rounded-md border border-stroke-soft bg-panel">
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
