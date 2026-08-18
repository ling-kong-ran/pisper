// 工作流页面：列表视图 + 编辑器（路由 /workflows/:id）的宿主，
// 管理工作流目录、保存与运行，并向壳层注册主操作。
import { useCallback, useEffect, useState } from 'react'
import '@xyflow/react/dist/style.css'
import { AlertTriangle, RefreshCw } from 'lucide-react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { PAGE_PATHS, workflowPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { Alert, AlertDescription } from '@/components/ui/alert'
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

import { AppEmptyState } from '@/components/ui/app-primitives'

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
    <AppEmptyState
      size="sm"
      className="workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] gap-2 py-4"
    >
      <RefreshCw className="animate-spin" size={23} />
      <h2>{label}</h2>
    </AppEmptyState>
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
    <div className="workflows-page flex min-h-[100%] flex-col gap-[12px]">
      <WorkflowError message={catalog.error} />
      <div className="workflow-page-toolbar max-[650px]:items-stretch max-[650px]:flex-col max-[650px]:gap-[8px] flex min-w-0 items-center justify-between gap-[16px]">
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
    <div className="preview-page flex min-h-[100%] flex-col workflow-editor-page">
      <WorkflowError message={editor.error} />
      {editor.running && editor.currentRun && (
        <WorkflowRunningNotice run={editor.currentRun} t={t} />
      )}
      <div className="builder-layout [.preview-page_>_&]:min-h-0 [.preview-page_>_&]:flex-1 max-[1150px]:grid-cols-[180px_minmax(460px,1fr)] max-[900px]:grid-cols-[180px_minmax(520px,1fr)] max-[900px]:overflow-auto max-[650px]:flex max-[650px]:min-w-0 max-[650px]:flex-col max-[650px]:overflow-visible max-[650px]:[.page-workflowCreate_&]:w-[900px] grid min-h-[100%] grid-cols-[205px_minmax(480px,1fr)_300px] gap-[12px]">
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
          systemNotificationPermission={editor.systemNotificationPermission}
          onUpdateDraft={editor.updateDraft}
          onUpdateNode={editor.updateNode}
          onToggleNotification={editor.toggleNotification}
          onDeleteEdge={editor.removeSelectedEdge}
          onCopyNode={editor.copyNode}
          onDeleteNode={editor.deleteNode}
          onOpenChannels={() => navigate(PAGE_PATHS.channels)}
          onOpenSystemNotificationSettings={() => {
            if (window.pisperDesktop?.openNotificationSettings) {
              void window.pisperDesktop.openNotificationSettings()
              return
            }
            navigate('/config/notifications')
          }}
        />
      </div>
    </div>
  )
}
