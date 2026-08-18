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
      className={cn(
        'flow-node [.workflow-mini-map_&.compact]:w-[100px] [.workflow-mini-map_&.compact]:min-h-[42px] [.workflow-mini-map_&.compact]:p-[5px_8px] [.workflow-mini-map_&.compact]:cursor-default [.workflow-mini-map_&.compact_small]:text-[11px] [.workflow-mini-map_&.compact_strong]:max-w-[100%] [.workflow-mini-map_&.compact_strong]:overflow-hidden [.workflow-mini-map_&.compact_strong]:text-[12px] [.workflow-mini-map_&.compact_strong]:text-ellipsis [.workflow-mini-map_&.compact_strong]:whitespace-nowrap [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_strong]:text-[13px] [&.active]:border-[var(--star)] [&.active]:[animation:star-node-pulse_2.4s_var(--ease-out)_infinite] [&.type-condition]:border-[var(--warning-border)] [&.type-condition]:bg-[var(--warning-subtle)] [&.type-parallel]:border-[var(--violet-border)] [&.type-parallel]:bg-[var(--violet-soft)] [&.type-approval]:border-[var(--approval-border)] [&.type-approval]:bg-[var(--success-subtle)] [&.type-notification]:border-[var(--notification-border)] [&.type-notification]:bg-[var(--notification-soft)] dark:[&.type-condition]:bg-[var(--warning-soft)] dark:[&.type-并行]:bg-[var(--violet-soft)] dark:[&.type-审批]:bg-[var(--success-subtle)] dark:[&.type-通知]:bg-[var(--notification-soft)] relative flex w-[120px] min-h-[49px] flex-col items-start justify-center gap-[3px] [border:1px_solid_var(--accent-border)] rounded-[var(--r-sm)] bg-[var(--accent-soft)] [padding:7px_10px] text-left shadow-[0_8px_18px_-14px_var(--node-shadow)] cursor-grab compact',
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
        style: { stroke: 'var(--canvas-edge)', strokeWidth: 2 },
      })),
    [edges],
  )

  return (
    <div className="workflow-mini-map relative h-[150px] overflow-hidden [margin-top:5px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--canvas-bg)]">
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
