// 节点库和画布共享编辑器回调；节点库弹层不占用画布布局空间。
import { useState } from 'react'
import { GripVertical, Plus, Search } from 'lucide-react'
import { isWorkflowImageNodeKind } from '@shared/workflow-image-nodes.mjs'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { WorkflowCanvas } from './WorkflowCanvas'
import type { NodeKind, Workflow } from './types'
import {
  nodeTypeLabel,
  paletteLabel,
  WORKFLOW_PALETTE,
  type WorkflowTranslate,
} from './workflow-templates'

export function WorkflowNodePalette({
  nodeCount,
  t,
  onAddNode,
}: {
  nodeCount: number
  t: WorkflowTranslate
  onAddNode: (kind: NodeKind, label: string, position: { x: number; y: number }) => void
}) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const groups = [
    {
      label: t('workflows:editor.flowNodes'),
      match: (kind: NodeKind) => ['trigger', 'condition', 'parallel', 'approval'].includes(kind),
    },
    {
      label: t('workflows:editor.agentNodes'),
      match: (kind: NodeKind) => ['prompt', 'skill', 'file', 'mcp', 'notification'].includes(kind),
    },
    { label: t('workflows:editor.imageNodes'), match: isWorkflowImageNodeKind },
  ]
  const visible = WORKFLOW_PALETTE.filter(({ kind }) =>
    paletteLabel(kind, t).toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
  )
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        {/* 编辑器页头里的工具键：手机只留图标（读屏名称不变），触控高度由页头统一抬到 44。 */}
        <Button variant="outline" size="sm" title={t('workflows:editor.addNode')}>
          <Plus />
          <span className="max-[650px]:sr-only">{t('workflows:editor.addNode')}</span>
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="w-72 max-w-[calc(100vw-2rem)] p-1.5"
        aria-label={t('workflows:workflowsPage.nodeLibrary')}
      >
        <div className="relative">
          <Search className="pointer-events-none absolute top-2.5 left-2.5 size-4 text-content-muted" />
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            className="pl-8"
            placeholder={t('workflows:editor.searchNodes')}
            aria-label={t('workflows:editor.searchNodes')}
          />
        </div>
        <div className="node-library max-h-[min(420px,60vh)] overflow-y-auto overscroll-contain supports-[height:100dvh]:max-h-[min(420px,60dvh)]">
          {groups.map((group) => {
            const items = visible.filter(({ kind }) => group.match(kind))
            if (items.length === 0) return null
            return (
              <section key={group.label} className="py-1">
                <h3 className="px-2 py-1.5 text-xs font-medium text-content-muted">
                  {group.label}
                </h3>
                {items.map(({ kind, Icon }) => (
                  <button
                    key={kind}
                    type="button"
                    draggable
                    // 节点库行与菜单同一几何：30px 行、r-sm，悬停只染一级表面；焦点沿用全局 2px 焦点描边。
                    className="flex min-h-[30px] w-full cursor-grab items-center gap-2 rounded-sm px-2 text-left text-sm text-content transition-colors duration-(--d-fast) hover:bg-surface-hover focus-visible:-outline-offset-2 max-[650px]:min-h-11 pointer-coarse:min-h-11"
                    onClick={() => {
                      onAddNode(kind, paletteLabel(kind, t), {
                        x: 160 + (nodeCount % 4) * 180,
                        y: 100 + Math.floor(nodeCount / 4) * 100,
                      })
                      setOpen(false)
                    }}
                    onDragStart={(event) =>
                      event.dataTransfer.setData(
                        'text/plain',
                        JSON.stringify({ kind, label: paletteLabel(kind, t) }),
                      )
                    }
                    onDragEnd={() => setOpen(false)}
                  >
                    <Icon className="size-4 shrink-0 text-content-muted" />
                    <span className="flex-1">{paletteLabel(kind, t)}</span>
                    <GripVertical className="size-3.5 text-content-faint" aria-hidden="true" />
                  </button>
                ))}
              </section>
            )
          })}
          {visible.length === 0 && (
            <p className="p-3 text-xs text-content-muted">
              {t('workflows:editor.noMatchingNodes')}
            </p>
          )}
        </div>
      </PopoverContent>
    </Popover>
  )
}

export function WorkflowEditorCanvas({
  draft,
  selectedNodeId,
  selectedEdgeId,
  nodeStatus,
  t,
  onAddNode,
  onConnect,
  onMoveNode,
  onSelectNode,
  onSelectEdge,
  onClearSelection,
  onDeleteNodes,
  onDeleteEdges,
}: {
  draft: Workflow
  selectedNodeId: string
  selectedEdgeId: string
  nodeStatus?: ReadonlyMap<string, string>
  t: WorkflowTranslate
  onAddNode: (kind: NodeKind, label: string, position: { x: number; y: number }) => void
  onConnect: (source: string, target: string, sourcePort: string) => void
  onMoveNode: (id: string, position: { x: number; y: number }) => void
  onSelectNode: (id: string) => void
  onSelectEdge: (id: string) => void
  onClearSelection: () => void
  onDeleteNodes: (ids: string[]) => void
  onDeleteEdges: (ids: string[]) => void
}) {
  return (
    <section
      className="builder-canvas relative h-full min-h-0 min-w-0 overflow-hidden bg-panel"
      aria-label={t('workflows:editor.canvas')}
    >
      <WorkflowCanvas
        nodes={draft.nodes}
        edges={draft.edges}
        selectedNodeId={selectedNodeId}
        selectedEdgeId={selectedEdgeId}
        nodeStatus={nodeStatus}
        hint={t('workflows:workflowsPage.dragFromANodeOutputToTheTargetInputToConnectThem')}
        inputLabel={t('workflows:workflowsPage.inputPort')}
        outputLabel={t('workflows:workflowsPage.outputPort')}
        nodeTypeLabel={(kind) => nodeTypeLabel(kind, t)}
        onAddNode={onAddNode}
        onConnect={onConnect}
        onMoveNode={onMoveNode}
        onSelectNode={onSelectNode}
        onSelectEdge={onSelectEdge}
        onClearSelection={onClearSelection}
        onDeleteNodes={onDeleteNodes}
        onDeleteEdges={onDeleteEdges}
      />
    </section>
  )
}
