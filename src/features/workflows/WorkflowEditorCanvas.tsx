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
      <Card size="sm" className="workflow-card node-library gap-0 py-0">
        <CardContent className="p-3.5">
          <CardTitle className="workflow-section-title">
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
      <Card size="sm" className="workflow-card builder-canvas gap-0 py-0">
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
