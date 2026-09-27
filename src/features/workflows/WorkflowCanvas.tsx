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
        'flow-node relative flex w-40 min-h-16 cursor-grab flex-col justify-center gap-1 rounded-xl border border-border bg-card px-3 py-2.5 text-left shadow-sm transition-shadow [&_small]:text-[11px] [&_small]:font-normal [&_small]:text-muted-foreground [&_strong]:text-[13px] [&_strong]:font-medium [&.active]:border-primary [&.active]:ring-2 [&.active]:ring-primary/15 [&.type-condition]:border-amber-500/40 [&.type-parallel]:border-violet-500/40 [&.type-approval]:border-emerald-500/40 [&.compact]:w-28 [&.compact]:min-h-10 [&.compact]:px-2 [&.compact]:py-1',
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
        fitViewOptions={{
          padding: 0.18,
          minZoom: 0.6,
          maxZoom: 1,
          // 复杂模板先从输入端以可读比例进入；缩放控件仍可一键查看完整图。
          nodes: flowNodes.length > 8 ? flowNodes.slice(0, 3).map(({ id }) => ({ id })) : undefined,
        }}
        minZoom={0.15}
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
          className="workflow-react-flow-minimap !m-3 !h-24 !w-36 overflow-hidden rounded-lg border border-border !bg-card shadow-sm [&_svg]:h-full [&_svg]:w-full @max-[640px]/workflow:hidden"
          nodeColor={nodeColor}
          nodeStrokeWidth={3}
          pannable
          zoomable
        />
        <Controls position="top-left" showInteractive={false} />
      </ReactFlow>
      <div className="pointer-events-none absolute bottom-3 left-3 z-[5] hidden max-w-[calc(100%-180px)] rounded-md bg-card/90 px-2 py-1 text-[11px] leading-relaxed text-muted-foreground @min-[640px]/workflow:block">
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
