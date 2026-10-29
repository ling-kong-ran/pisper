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

import { Check, Clock3, X, type LucideIcon } from 'lucide-react'
import { Controls } from '@/components/ai-elements/controls'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'

import { WORKFLOW_NODE_KINDS, type NodeKind, type WorkflowEdge, type WorkflowNode } from './types'
import { WORKFLOW_PALETTE } from './workflow-templates'

type WorkflowCanvasNodeData = {
  kind: NodeKind
  label: string
  typeLabel: string
  inputLabel: string
  outputLabel: string
  status?: string
}

type WorkflowFlowNode = Node<WorkflowCanvasNodeData, 'workflow'>
type WorkflowFlowEdge = Edge<Record<string, never>, 'default'>

type WorkflowCanvasProps = {
  nodes: WorkflowNode[]
  edges: WorkflowEdge[]
  selectedNodeId: string
  selectedEdgeId: string
  // 仅在运行进行中传入：节点 id → 本次运行中的节点状态，用于在画布上标出运行位置。
  nodeStatus?: ReadonlyMap<string, string>
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

const NODE_ICONS = new Map<NodeKind, LucideIcon>(
  WORKFLOW_PALETTE.map(({ kind, Icon }) => [kind, Icon]),
)
// 端口：面板色实心圆 + 控件描边（≥3:1），悬停/连接时换成焦点色；::after 把命中区扩到 28px。
const PORT_CLASS =
  'flow-port size-2.5! rounded-full! border-[1.5px]! border-control! bg-panel! cursor-crosshair after:absolute after:-inset-2 hover:border-focus!'

// 运行状态字形：形状与颜色共同表达（勾 / 叉 / 时钟 / 轨道），与对话里的工具日志一致。
function NodeStatusGlyph({ status }: { status?: string }) {
  if (status === 'running') return <OrbitSpinner className="ml-auto size-3.5" />
  if (status === 'completed')
    return <Check className="ml-auto size-3.5 text-success" strokeWidth={2.25} />
  if (status === 'failed') return <X className="ml-auto size-3.5 text-danger" strokeWidth={2.25} />
  if (status === 'waiting_approval')
    return <Clock3 className="ml-auto size-3.5 text-warning-strong" />
  return null
}

function WorkflowNodeCard({ data, selected }: NodeProps<WorkflowFlowNode>) {
  const Icon = NODE_ICONS.get(data.kind)
  return (
    <div
      className={cn(
        // 默认节点是平铺在细线后的卡片；选中节点抬起为键帽并带 3px 强调色刻度，运行中的节点换成强调色描边。
        // React Flow 会去掉外层节点的焦点轮廓，键盘焦点改画在卡片上，与选中键帽区分。
        'flow-node relative flex w-36 min-h-14 cursor-grab flex-col justify-center gap-0.5 rounded-md border border-stroke bg-panel px-3 py-2 text-left transition-[border-color,box-shadow] duration-(--d-fast) ease-precise [:focus-visible>&]:outline-2 [:focus-visible>&]:outline-offset-2 [:focus-visible>&]:outline-focus',
        `type-${data.kind}`,
        selected && 'active border-stroke-strong bg-key shadow-key',
        data.status === 'running' && 'border-star-border',
      )}
    >
      {selected && (
        <i
          aria-hidden="true"
          className="absolute inset-y-2 -left-px w-[3px] rounded-r-[3px] bg-star"
        />
      )}
      {data.kind !== 'trigger' && (
        <Handle
          id="input"
          className={`${PORT_CLASS} input`}
          type="target"
          position={Position.Left}
          title={data.inputLabel}
          aria-label={data.inputLabel}
        />
      )}
      {data.kind === 'condition' ? (
        <>
          <Handle
            id="true"
            className={`${PORT_CLASS} output condition-true`}
            type="source"
            position={Position.Right}
            title="true"
            aria-label="true"
          />
          <Handle
            id="false"
            className={`${PORT_CLASS} output condition-false`}
            type="source"
            position={Position.Bottom}
            title="false"
            aria-label="false"
          />
        </>
      ) : (
        <Handle
          id="output"
          className={`${PORT_CLASS} output`}
          type="source"
          position={Position.Right}
          title={data.outputLabel}
          aria-label={data.outputLabel}
        />
      )}
      <span className="flex min-w-0 items-center gap-1.5 text-xs text-content-muted">
        {Icon && <Icon className="size-3.5 shrink-0" aria-hidden="true" />}
        <span className="truncate">{data.typeLabel}</span>
        <NodeStatusGlyph status={data.status} />
      </span>
      <strong className="truncate text-sm font-medium text-content" title={data.label}>
        {data.label}
      </strong>
    </div>
  )
}

const NODE_TYPES: NodeTypes = { workflow: WorkflowNodeCard }

function isNodeKind(value: unknown): value is NodeKind {
  return typeof value === 'string' && WORKFLOW_NODE_KINDS.includes(value as NodeKind)
}

// 小地图保持单色：节点一律描边灰，只有正在运行的节点用强调色标出位置。
function nodeColor(node: WorkflowFlowNode) {
  return node.data.status === 'running' ? 'var(--star)' : 'var(--stroke-strong)'
}

function WorkflowCanvasInner({
  nodes,
  edges,
  selectedNodeId,
  selectedEdgeId,
  nodeStatus,
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
          status: nodeStatus?.get(node.id),
        },
      })),
    [inputLabel, nodeStatus, nodeTypeLabel, nodes, outputLabel, selectedNodeId],
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
          currentNode.data.status === externalNode.data.status
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
        // 线色与粗细交给画布的 --xy-edge-* 变量，选中态才能换成强调色；箭头保持淡墨色。
        markerEnd: {
          type: MarkerType.ArrowClosed,
          width: 14,
          height: 14,
          color: 'var(--canvas-edge)',
        },
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
        <Background variant={BackgroundVariant.Dots} gap={20} size={1} color="var(--canvas-grid)" />
        <MiniMap
          className="workflow-react-flow-minimap m-3! h-24! w-36! overflow-hidden rounded-md border border-stroke [&_svg]:h-full [&_svg]:w-full @max-[640px]/workflow:hidden"
          nodeColor={nodeColor}
          nodeBorderRadius={4}
          pannable
          zoomable
        />
        {/* 缩放控件是一组 28px 图标键；手机与粗指针下放大到 44px 触控目标。 */}
        <Controls
          position="top-left"
          showInteractive={false}
          className="border-stroke bg-panel p-0.5 [&>button]:size-7! [&>button]:rounded-sm! [&>button]:hover:bg-surface-hover! max-[650px]:[&>button]:size-11! pointer-coarse:[&>button]:size-11!"
        />
      </ReactFlow>
      <div className="pointer-events-none absolute bottom-3 left-3 z-[5] hidden max-w-[calc(100%-180px)] text-xs text-content-muted @min-[640px]/workflow:block">
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
