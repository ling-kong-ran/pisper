// 工作流画布：基于 @xyflow/react 的节点/连线编辑器，支持拖拽建边、
// 节点拖放与缩放，以及节点选中联动检查器。
import { useCallback, useEffect, useMemo, useRef, type DragEvent } from 'react'
import {
  Background,
  BackgroundVariant,
  ConnectionMode,
  Handle,
  MarkerType,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useNodesState,
  useReactFlow,
  type Connection,
  type Edge,
  type Node,
  type NodeProps,
  type NodeTypes,
} from '@xyflow/react'

import { Controls } from '@/components/ai-elements/controls'
import { cn } from '@/lib/utils'

import { WORKFLOW_NODE_KINDS, type NodeKind, type WorkflowEdge, type WorkflowNode } from './types'

type WorkflowCanvasNodeData = {
  kind: NodeKind
  label: string
  typeLabel: string
  inputLabel: string
  outputLabel: string
  compact?: boolean
}

type WorkflowFlowNode = Node<WorkflowCanvasNodeData, 'workflow'>
type WorkflowFlowEdge = Edge<Record<string, never>, 'default'>

type WorkflowCanvasProps = {
  nodes: WorkflowNode[]
  edges: WorkflowEdge[]
  selectedNodeId: string
  selectedEdgeId: string
  hint: string
  inputLabel: string
  outputLabel: string
  nodeTypeLabel: (kind: NodeKind) => string
  onAddNode: (kind: NodeKind, label: string, position: { x: number; y: number }) => void
  onConnect: (source: string, target: string, sourcePort: string) => void
  onMoveNode: (id: string, position: { x: number; y: number }) => void
  onSelectNode: (id: string) => void
  onSelectEdge: (id: string) => void
  onClearSelection: () => void
  onDeleteNodes: (ids: string[]) => void
  onDeleteEdges: (ids: string[]) => void
}

function WorkflowNodeCard({ data, selected }: NodeProps<WorkflowFlowNode>) {
  return (
    <div
      className={cn(
        'flow-node [.workflow-mini-map_&.compact]:w-[100px] [.workflow-mini-map_&.compact]:min-h-[42px] [.workflow-mini-map_&.compact]:p-[5px_8px] [.workflow-mini-map_&.compact]:cursor-default [.workflow-mini-map_&.compact_small]:text-[11px] [.workflow-mini-map_&.compact_strong]:max-w-[100%] [.workflow-mini-map_&.compact_strong]:overflow-hidden [.workflow-mini-map_&.compact_strong]:text-[12px] [.workflow-mini-map_&.compact_strong]:text-ellipsis [.workflow-mini-map_&.compact_strong]:whitespace-nowrap [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_strong]:text-[13px] [&.active]:border-[var(--star)] [&.active]:[animation:star-node-pulse_2.4s_var(--ease-out)_infinite] [&.type-condition]:border-[var(--warning-border)] [&.type-condition]:bg-[var(--warning-subtle)] [&.type-parallel]:border-[var(--violet-border)] [&.type-parallel]:bg-[var(--violet-soft)] [&.type-approval]:border-[var(--approval-border)] [&.type-approval]:bg-[var(--success-subtle)] [&.type-notification]:border-[var(--notification-border)] [&.type-notification]:bg-[var(--notification-soft)] dark:[&.type-condition]:bg-[var(--warning-soft)] dark:[&.type-并行]:bg-[var(--violet-soft)] dark:[&.type-审批]:bg-[var(--success-subtle)] dark:[&.type-通知]:bg-[var(--notification-soft)] relative flex w-[120px] min-h-[49px] flex-col items-start justify-center gap-[3px] [border:1px_solid_var(--accent-border)] rounded-[var(--r-sm)] bg-[var(--accent-soft)] [padding:7px_10px] text-left shadow-[0_8px_18px_-14px_var(--node-shadow)] cursor-grab',
        `type-${data.kind}`,
        selected && 'active',
        data.compact && 'compact',
      )}
    >
      {!data.compact && data.kind !== 'trigger' && (
        <Handle
          id="input"
          className="flow-port after:absolute after:[content:''] after:inset-[-8px] absolute z-[3] w-[12px] h-[12px] [border:2px_solid_var(--solid)] rounded-[50%] bg-[var(--text)] [cursor:crosshair] input [.flow-port&]:top-[50%] [.flow-port&]:left-[-7px] [.flow-port&]:[transform:translateY(-50%)]"
          type="target"
          position={Position.Left}
          title={data.inputLabel}
          aria-label={data.inputLabel}
        />
      )}
      {!data.compact && data.kind === 'condition' ? (
        <>
          <Handle
            id="true"
            className="flow-port after:absolute after:[content:''] after:inset-[-8px] absolute z-[3] w-[12px] h-[12px] [border:2px_solid_var(--solid)] rounded-[50%] bg-[var(--text)] [cursor:crosshair] output [.flow-port&]:top-[50%] [.flow-port&]:right-[-7px] [.flow-port&]:[transform:translateY(-50%)] condition-true"
            type="source"
            position={Position.Right}
            title="true"
            aria-label="true"
          />
          <Handle
            id="false"
            className="flow-port after:absolute after:[content:''] after:inset-[-8px] absolute z-[3] w-[12px] h-[12px] [border:2px_solid_var(--solid)] rounded-[50%] bg-[var(--text)] [cursor:crosshair] output [.flow-port&]:top-[50%] [.flow-port&]:right-[-7px] [.flow-port&]:[transform:translateY(-50%)] condition-false"
            type="source"
            position={Position.Bottom}
            title="false"
            aria-label="false"
          />
        </>
      ) : (
        !data.compact && (
          <Handle
            id="output"
            className="flow-port after:absolute after:[content:''] after:inset-[-8px] absolute z-[3] w-[12px] h-[12px] [border:2px_solid_var(--solid)] rounded-[50%] bg-[var(--text)] [cursor:crosshair] output [.flow-port&]:top-[50%] [.flow-port&]:right-[-7px] [.flow-port&]:[transform:translateY(-50%)]"
            type="source"
            position={Position.Right}
            title={data.outputLabel}
            aria-label={data.outputLabel}
          />
        )
      )}
      <small>{data.typeLabel}</small>
      <strong>{data.label}</strong>
    </div>
  )
}

const NODE_TYPES: NodeTypes = { workflow: WorkflowNodeCard }

function isNodeKind(value: unknown): value is NodeKind {
  return typeof value === 'string' && WORKFLOW_NODE_KINDS.includes(value as NodeKind)
}

function nodeColor(node: WorkflowFlowNode) {
  if (node.data.kind === 'condition') return 'var(--warning-strong)'
  if (node.data.kind === 'parallel') return 'var(--workflow-violet)'
  if (node.data.kind === 'approval') return 'var(--success)'
  if (node.data.kind === 'notification') return 'var(--notification-border)'
  return 'var(--star-strong)'
}

function WorkflowCanvasInner({
  nodes,
  edges,
  selectedNodeId,
  selectedEdgeId,
  hint,
  inputLabel,
  outputLabel,
  nodeTypeLabel,
  onAddNode,
  onConnect,
  onMoveNode,
  onSelectNode,
  onSelectEdge,
  onClearSelection,
  onDeleteNodes,
  onDeleteEdges,
}: WorkflowCanvasProps) {
  const { screenToFlowPosition } = useReactFlow<WorkflowFlowNode, WorkflowFlowEdge>()

  const externalFlowNodes = useMemo<WorkflowFlowNode[]>(
    () =>
      nodes.map((node) => ({
        id: node.id,
        type: 'workflow',
        position: { x: node.x, y: node.y },
        selected: node.id === selectedNodeId,
        data: {
          kind: node.kind,
          label: node.label,
          typeLabel: nodeTypeLabel(node.kind),
          inputLabel,
          outputLabel,
        },
      })),
    [inputLabel, nodeTypeLabel, nodes, outputLabel, selectedNodeId],
  )
  const draggingNodeIds = useRef(new Set<string>())
  const pendingNodePositions = useRef(new Map<string, { x: number; y: number }>())
  const moveFrame = useRef<number | null>(null)
  const [flowNodes, setFlowNodes, handleNodesChange] =
    useNodesState<WorkflowFlowNode>(externalFlowNodes)

  useEffect(() => {
    setFlowNodes((currentNodes) => {
      const currentById = new Map(currentNodes.map((node) => [node.id, node]))
      let changed = currentNodes.length !== externalFlowNodes.length
      const nextNodes = externalFlowNodes.map((externalNode) => {
        const currentNode = currentById.get(externalNode.id)
        if (!currentNode) {
          changed = true
          return externalNode
        }

        const position = draggingNodeIds.current.has(externalNode.id)
          ? currentNode.position
          : externalNode.position
        const dataUnchanged =
          currentNode.data.kind === externalNode.data.kind &&
          currentNode.data.label === externalNode.data.label &&
          currentNode.data.typeLabel === externalNode.data.typeLabel &&
          currentNode.data.inputLabel === externalNode.data.inputLabel &&
          currentNode.data.outputLabel === externalNode.data.outputLabel &&
          currentNode.data.compact === externalNode.data.compact
        const nodeUnchanged =
          position.x === currentNode.position.x &&
          position.y === currentNode.position.y &&
          currentNode.selected === externalNode.selected &&
          dataUnchanged

        if (nodeUnchanged) return currentNode
        changed = true
        return {
          ...currentNode,
          position,
          selected: externalNode.selected,
          data: externalNode.data,
        }
      })
      return changed ? nextNodes : currentNodes
    })
  }, [externalFlowNodes, setFlowNodes])

  // 批量提交节点位置：把 rAF 周期内收集的位置变化一次回调，
  // 避免拖拽过程中每个像素都触发一次状态写回。
  const flushNodePositions = useCallback(() => {
    moveFrame.current = null
    pendingNodePositions.current.forEach((position, id) => onMoveNode(id, position))
    pendingNodePositions.current.clear()
  }, [onMoveNode])

  // 记录节点位置：先入 pending 表，再调度一帧批量回调（rAF 合并）。
  const syncNodePosition = useCallback(
    (id: string, position: { x: number; y: number }) => {
      pendingNodePositions.current.set(id, position)
      moveFrame.current ??= requestAnimationFrame(flushNodePositions)
    },
    [flushNodePositions],
  )

  useEffect(
    () => () => {
      if (moveFrame.current !== null) cancelAnimationFrame(moveFrame.current)
    },
    [],
  )

  const flowEdges = useMemo<WorkflowFlowEdge[]>(
    () =>
      edges.map((edge) => ({
        id: edge.id,
        source: edge.source,
        sourceHandle: edge.sourcePort,
        target: edge.target,
        targetHandle: edge.targetPort,
        selected: edge.id === selectedEdgeId,
        markerEnd: {
          type: MarkerType.ArrowClosed,
          width: 16,
          height: 16,
          color: 'var(--canvas-edge)',
        },
        style: { stroke: 'var(--canvas-edge)', strokeWidth: 2 },
      })),
    [edges, selectedEdgeId],
  )

  // 连线完成回调：校验两端后转发给编辑器建立边。
  const handleConnect = useCallback(
    (connection: Connection) => {
      if (!connection.source || !connection.target) return
      onConnect(connection.source, connection.target, connection.sourceHandle || 'output')
    },
    [onConnect],
  )

  const handleDragOver = useCallback((event: DragEvent<HTMLDivElement>) => {
    event.preventDefault()
    event.dataTransfer.dropEffect = 'move'
  }, [])

  // 拖放新建节点：解析拖拽负载（kind/label），按鼠标位置映射为画布坐标。
  const handleDrop = useCallback(
    (event: DragEvent<HTMLDivElement>) => {
      event.preventDefault()
      let payload: { kind?: unknown; label?: unknown } = {}
      try {
        payload = JSON.parse(event.dataTransfer.getData('text/plain') || '{}')
      } catch {
        return
      }
      if (!isNodeKind(payload.kind)) return
      const position = screenToFlowPosition({ x: event.clientX, y: event.clientY })
      onAddNode(
        payload.kind,
        typeof payload.label === 'string' ? payload.label : nodeTypeLabel(payload.kind),
        position,
      )
    },
    [nodeTypeLabel, onAddNode, screenToFlowPosition],
  )

  return (
    <div className="workflow-react-flow absolute inset-0">
      <ReactFlow<WorkflowFlowNode, WorkflowFlowEdge>
        nodes={flowNodes}
        edges={flowEdges}
        nodeTypes={NODE_TYPES}
        onNodesChange={handleNodesChange}
        connectionMode={ConnectionMode.Strict}
        deleteKeyCode={['Backspace', 'Delete']}
        fitView
        fitViewOptions={{ padding: 0.18, minZoom: 0.55, maxZoom: 1 }}
        minZoom={0.35}
        maxZoom={1.8}
        snapToGrid
        snapGrid={[20, 20]}
        panOnDrag
        zoomOnDoubleClick={false}
        onConnect={handleConnect}
        onNodeClick={(_event, node) => onSelectNode(node.id)}
        onEdgeClick={(_event, edge) => onSelectEdge(edge.id)}
        onPaneClick={onClearSelection}
        onNodeDragStart={(_event, node) => draggingNodeIds.current.add(node.id)}
        onNodeDrag={(_event, node) => syncNodePosition(node.id, node.position)}
        onNodeDragStop={(_event, node) => {
          draggingNodeIds.current.delete(node.id)
          pendingNodePositions.current.delete(node.id)
          onMoveNode(node.id, node.position)
        }}
        onNodesDelete={(deleted) => onDeleteNodes(deleted.map((node) => node.id))}
        onEdgesDelete={(deleted) => onDeleteEdges(deleted.map((edge) => edge.id))}
        onDragOver={handleDragOver}
        onDrop={handleDrop}
      >
        <Background
          variant={BackgroundVariant.Dots}
          gap={20}
          size={1.2}
          color="var(--canvas-grid)"
        />
        <MiniMap
          className="workflow-react-flow-minimap [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] !bg-[var(--surface-subtle)]"
          nodeColor={nodeColor}
          nodeStrokeWidth={3}
          pannable
          zoomable
        />
        <Controls position="top-left" showInteractive={false} />
      </ReactFlow>
      <div className="absolute z-[5] [left:12px] [bottom:10px] text-[var(--text-muted)] text-[12px] pointer-events-none">
        {hint}
      </div>
    </div>
  )
}

export function WorkflowCanvas(props: WorkflowCanvasProps) {
  return (
    <ReactFlowProvider>
      <WorkflowCanvasInner {...props} />
    </ReactFlowProvider>
  )
}
