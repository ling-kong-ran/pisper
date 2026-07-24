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
        'flow-node',
        `type-${data.kind}`,
        selected && 'active',
        data.compact && 'compact',
      )}
    >
      {!data.compact && data.kind !== 'trigger' && (
        <Handle
          id="input"
          className="flow-port input"
          type="target"
          position={Position.Left}
          title={data.inputLabel}
          aria-label={data.inputLabel}
        />
      )}
      {!data.compact && (
        <Handle
          id="output"
          className="flow-port output"
          type="source"
          position={Position.Right}
          title={data.outputLabel}
          aria-label={data.outputLabel}
        />
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

  const flushNodePositions = useCallback(() => {
    moveFrame.current = null
    pendingNodePositions.current.forEach((position, id) => onMoveNode(id, position))
    pendingNodePositions.current.clear()
  }, [onMoveNode])

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
    <div className="workflow-react-flow">
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
          className="workflow-react-flow-minimap"
          nodeColor={nodeColor}
          nodeStrokeWidth={3}
          pannable
          zoomable
        />
        <Controls position="top-left" showInteractive={false} />
      </ReactFlow>
      <div className="canvas-hint">{hint}</div>
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

export function WorkflowPreview({
  nodes,
  edges,
  nodeTypeLabel,
}: {
  nodes: WorkflowNode[]
  edges: WorkflowEdge[]
  nodeTypeLabel: (kind: NodeKind) => string
}) {
  const previewNodes = useMemo<WorkflowFlowNode[]>(
    () =>
      nodes.map((node) => ({
        id: node.id,
        type: 'workflow',
        position: { x: node.x, y: node.y },
        data: {
          kind: node.kind,
          label: node.label,
          typeLabel: nodeTypeLabel(node.kind),
          inputLabel: '',
          outputLabel: '',
          compact: true,
        },
      })),
    [nodeTypeLabel, nodes],
  )
  const previewEdges = useMemo<WorkflowFlowEdge[]>(
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
      <ReactFlow<WorkflowFlowNode, WorkflowFlowEdge>
        nodes={previewNodes}
        edges={previewEdges}
        nodeTypes={NODE_TYPES}
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
