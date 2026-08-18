// 工作流编辑器画布：节点列表侧栏 + 画布 + 属性检查器的编排布局。
import { GripVertical } from 'lucide-react'
import { Card, CardContent, CardTitle } from '@/components/ui/card'
import { WorkflowCanvas } from './WorkflowCanvas'
import type { NodeKind, Workflow } from './types'
import {
  nodeTypeLabel,
  paletteLabel,
  WORKFLOW_PALETTE,
  type WorkflowTranslate,
} from './workflow-templates'

export function WorkflowEditorCanvas({
  draft,
  selectedNodeId,
  selectedEdgeId,
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
    <>
      <Card
        size="sm"
        className="workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] node-library [&_[data-slot='card-content']_>_div_>_small]:block [&_[data-slot='card-content']_>_div_>_small]:h-[15px] [&_[data-slot='card-content']_>_div_>_small]:text-[var(--text-muted)] [&_[data-slot='card-content']_>_div_>_small]:text-[13px] [&_[data-slot='card-content']_>_div_>_small]:font-[600] [&_[data-slot='card-content']_>_div_>_button]:grid [&_[data-slot='card-content']_>_div_>_button]:w-full [&_[data-slot='card-content']_>_div_>_button]:grid-cols-[auto_minmax(0,1fr)_auto] [&_[data-slot='card-content']_>_div_>_button]:items-center [&_[data-slot='card-content']_>_div_>_button]:gap-[7px] [&_[data-slot='card-content']_>_div_>_button]:border-0 [&_[data-slot='card-content']_>_div_>_button]:[border-top:1px_solid_var(--stroke-soft)] [&_[data-slot='card-content']_>_div_>_button]:bg-transparent [&_[data-slot='card-content']_>_div_>_button]:p-[8px_3px] [&_[data-slot='card-content']_>_div_>_button]:text-[var(--text)] [&_[data-slot='card-content']_>_div_>_button]:text-left [&_[data-slot='card-content']_>_div_>_button]:text-[12px] [&_[data-slot='card-content']_>_div_>_button]:cursor-grab [&_[data-slot='card-content']_>_div_>_button_span]:text-[var(--text-muted)] [&_[data-slot='card-content']_>_div_>_button_span]:text-[13px] max-[650px]:max-h-[220px] overflow-auto gap-0 py-0"
      >
        <CardContent className="p-3.5">
          <CardTitle className="workflow-section-title [.selection-list_&]:mb-[8px] [.node-library_&]:mb-[8px] text-[var(--text-soft)] text-[13px] font-[700] leading-[1.4]">
            {t('workflows:workflowsPage.nodeLibrary')}
          </CardTitle>
          {WORKFLOW_PALETTE.map(({ kind, label, Icon }) => (
            <div key={kind}>
              <small>{kind === 'trigger' ? t('workflows:workflowsPage.triggerGroup') : ''}</small>
              <button
                draggable
                onDragStart={(event) =>
                  event.dataTransfer.setData('text/plain', JSON.stringify({ kind, label }))
                }
              >
                <Icon size={15} />
                {paletteLabel(kind, t)}
                <span
                  title={t('workflows:workflowsPage.drag')}
                  aria-label={t('workflows:workflowsPage.drag')}
                >
                  <GripVertical size={13} />
                </span>
              </button>
            </div>
          ))}
        </CardContent>
      </Card>
      <Card
        size="sm"
        className="workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] builder-canvas relative min-h-[620px] overflow-hidden p-0 !bg-[var(--canvas-bg)] dark:bg-[var(--canvas-bg)] max-[650px]:min-w-[620px] gap-0 py-0"
      >
        <WorkflowCanvas
          nodes={draft.nodes}
          edges={draft.edges}
          selectedNodeId={selectedNodeId}
          selectedEdgeId={selectedEdgeId}
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
      </Card>
    </>
  )
}
