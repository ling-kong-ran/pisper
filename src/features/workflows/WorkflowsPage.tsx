import { useCallback, useEffect, useState } from 'react'
import '@xyflow/react/dist/style.css'
import { AlertTriangle, RefreshCw } from 'lucide-react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { PAGE_PATHS, workflowPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Card } from '@/components/ui/card'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { WorkflowEditorCanvas } from './WorkflowEditorCanvas'
import {
  WorkflowAssetList,
  WorkflowOperationsSummary,
  WorkflowRunHistory,
  WorkflowTemplateGallery,
  WorkflowViewTabs,
  type WorkflowView,
} from './WorkflowListSidebar'
import { WorkflowNodeInspector } from './WorkflowNodeInspector'
import { WorkflowRunningNotice } from './WorkflowRunControls'
import { useWorkflowCatalog } from './useWorkflowCatalog'
import { useWorkflowEditor } from './useWorkflowEditor'

type WorkflowsPageProps = {
  notify: Notify
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
  query?: string
}

type WorkflowBuilderProps = {
  notify: Notify
  registerPrimaryAction: (action: () => void | Promise<unknown>) => () => void
  registerWorkflowActions?: (actions: {
    save: () => void | Promise<unknown>
    run: () => void | Promise<unknown>
    busy: boolean
    running: boolean
  }) => () => void
}

function WorkflowError({ message }: { message: string }) {
  if (!message) return null
  return (
    <Alert variant="destructive">
      <AlertTriangle />
      <AlertDescription>{message}</AlertDescription>
    </Alert>
  )
}

function WorkflowLoading({ label }: { label: string }) {
  return (
    <Card size="sm" className="workflow-card empty-state gap-2 py-4">
      <RefreshCw className="spin" size={23} />
      <h2>{label}</h2>
    </Card>
  )
}

export function WorkflowsPage({ notify, requestConfirm, query = '' }: WorkflowsPageProps) {
  const { t, language } = useI18n()
  const navigate = useNavigate()
  const [view, setView] = useState<WorkflowView>('workflows')
  const catalog = useWorkflowCatalog({ notify, requestConfirm, query })

  if (catalog.loading) {
    return <WorkflowLoading label={t('workflows:workflowsPage.loadingWorkflows')} />
  }

  return (
    <div className="workflows-page">
      <WorkflowError message={catalog.error} />
      <div className="workflow-page-toolbar">
        <WorkflowViewTabs value={view} t={t} onChange={setView} />
        <WorkflowOperationsSummary data={catalog.data} t={t} />
      </div>
      {view === 'workflows' ? (
        <WorkflowAssetList
          workflows={catalog.visibleWorkflows}
          runs={catalog.data.runs}
          busyId={catalog.busyId}
          language={language}
          t={t}
          onRun={(workflow) => void catalog.runWorkflow(workflow)}
          onEdit={(workflowId) => navigate(workflowPath(workflowId))}
          onDuplicate={(workflow) => void catalog.duplicateWorkflow(workflow)}
          onExport={(workflow) => void catalog.exportWorkflow(workflow)}
          onImport={(value) => void catalog.importWorkflow(value)}
          onDelete={(workflow) => void catalog.removeWorkflow(workflow)}
        />
      ) : view === 'runs' ? (
        <WorkflowRunHistory
          runs={catalog.data.runs}
          busyId={catalog.busyId}
          language={language}
          t={t}
          onStop={(run) => void catalog.stopRun(run)}
          onRetry={(run) => void catalog.retryRun(run)}
          onApproval={(run, nodeId, approved) =>
            void catalog.resolveApproval(run, nodeId, approved)
          }
        />
      ) : (
        <WorkflowTemplateGallery
          t={t}
          onOpenTemplate={(templateId) =>
            navigate(`${workflowPath('new')}?template=${encodeURIComponent(templateId)}`)
          }
        />
      )}
    </div>
  )
}

export function WorkflowBuilder({
  notify,
  registerPrimaryAction,
  registerWorkflowActions,
}: WorkflowBuilderProps) {
  const { t, language } = useI18n()
  const navigate = useNavigate()
  const { workflowId = 'new' } = useParams()
  const [searchParams] = useSearchParams()
  const onCreated = useCallback(
    (createdWorkflowId: string) => {
      navigate(workflowPath(createdWorkflowId), { replace: true })
    },
    [navigate],
  )
  const editor = useWorkflowEditor({
    workflowId,
    templateId: searchParams.get('template'),
    notify,
    onCreated,
  })
  const { busy, publishWorkflow, runWorkflow, running, saveWorkflow, stopWorkflow } = editor

  usePagePrimaryAction(registerPrimaryAction, publishWorkflow)
  useEffect(
    () =>
      registerWorkflowActions?.({
        save: () => saveWorkflow('draft'),
        run: running ? stopWorkflow : runWorkflow,
        busy,
        running,
      }),
    [busy, registerWorkflowActions, runWorkflow, running, saveWorkflow, stopWorkflow],
  )

  if (editor.loading || !editor.draft) {
    return <WorkflowLoading label={t('workflows:workflowsPage.loadingWorkflowEditor')} />
  }

  return (
    <div className="preview-page workflow-editor-page">
      <WorkflowError message={editor.error} />
      {editor.running && editor.currentRun && (
        <WorkflowRunningNotice run={editor.currentRun} t={t} />
      )}
      <div className="builder-layout">
        <WorkflowEditorCanvas
          draft={editor.draft}
          selectedNodeId={editor.selectedNodeId}
          selectedEdgeId={editor.selectedEdgeId}
          t={t}
          onAddNode={editor.addNode}
          onConnect={editor.addEdge}
          onMoveNode={editor.moveNode}
          onSelectNode={editor.selectNode}
          onSelectEdge={editor.selectEdge}
          onClearSelection={editor.clearSelection}
          onDeleteNodes={editor.removeNodes}
          onDeleteEdges={editor.removeEdges}
        />
        <WorkflowNodeInspector
          draft={editor.draft}
          catalog={editor.catalog}
          selectedNode={editor.selectedNode}
          selectedEdge={editor.selectedEdge}
          currentRun={editor.currentRun}
          language={language}
          t={t}
          onUpdateDraft={editor.updateDraft}
          onUpdateNode={editor.updateNode}
          onToggleNotification={editor.toggleNotification}
          onDeleteEdge={editor.removeSelectedEdge}
          onCopyNode={editor.copyNode}
          onDeleteNode={editor.deleteNode}
          onOpenChannels={() => navigate(PAGE_PATHS.channels)}
        />
      </div>
    </div>
  )
}
