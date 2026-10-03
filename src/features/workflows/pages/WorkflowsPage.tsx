// 工作流页面：列表视图 + 编辑器（路由 /workflows/:id）的宿主，
// 管理工作流目录、保存与运行，并向壳层注册主操作。
import { useCallback, useEffect, useRef, useState } from 'react'
import '@xyflow/react/dist/style.css'
import {
  AlertTriangle,
  ArrowLeft,
  ArrowUpRight,
  Film,
  PanelRight,
  RefreshCw,
  Settings2,
} from 'lucide-react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { PAGE_PATHS, workflowPath } from '@/app/routes/routes'
import { useI18n } from '@/app/i18n/use-i18n'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { cn } from '@/lib/utils'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import type { Notify } from '@/app/routes/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { WorkflowEditorCanvas, WorkflowNodePalette } from '@/features/workflows/components/WorkflowEditorCanvas'
import {
  WorkflowAssetList,
  WorkflowOperationsSummary,
  WorkflowRunHistory,
  WorkflowTemplateGallery,
  WorkflowViewTabs,
  type WorkflowView,
} from '@/features/workflows/components/WorkflowListSidebar'
import { WorkflowNodeInspector, WorkflowSettings } from '@/features/workflows/components/WorkflowNodeInspector'
import { WorkflowRunningNotice } from '@/features/workflows/components/WorkflowRunControls'
import { useWorkflowCatalog } from '@/features/workflows/hooks/useWorkflowCatalog'
import { useWorkflowEditor } from '@/features/workflows/hooks/useWorkflowEditor'
import { WorkflowRunDialog } from '@/features/workflows/components/WorkflowRunDialog'
import type { Workflow } from '@/features/workflows/model/types'

import { AppEmptyState } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'

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
  const [searchParams, setSearchParams] = useSearchParams()
  const requestedView = searchParams.get('view')
  const view: WorkflowView =
    requestedView === 'runs' || requestedView === 'templates' ? requestedView : 'workflows'
  const openSpriteTemplate = () => navigate(`${workflowPath('new')}?template=sprite`)
  const setView = (next: WorkflowView) =>
    setSearchParams((current) => {
      const params = new URLSearchParams(current)
      if (next === 'workflows') params.delete('view')
      else params.set('view', next)
      return params
    })
  const catalog = useWorkflowCatalog({ notify, requestConfirm, query })
  const [runTarget, setRunTarget] = useState<Workflow | null>(null)

  if (catalog.loading) {
    return <WorkflowLoading label={t('workflows:workflowsPage.loadingWorkflows')} />
  }

  return (
    <div className="workflows-page flex min-h-[100%] flex-col gap-[12px]">
      {runTarget && (
        <WorkflowRunDialog
          workflow={runTarget}
          onClose={() => setRunTarget(null)}
          onRun={(inputs) => catalog.runWorkflow(runTarget, inputs)}
        />
      )}
      <WorkflowError message={catalog.error} />
      <div className="workflow-page-toolbar max-[650px]:items-stretch max-[650px]:flex-col max-[650px]:gap-[8px] flex min-w-0 items-center justify-between gap-[16px]">
        <WorkflowViewTabs value={view} t={t} onChange={setView} />
        <WorkflowOperationsSummary data={catalog.data} t={t} />
      </div>
      {view === 'workflows' && (
        <div className="flex flex-wrap items-center justify-between gap-4 rounded-2xl border bg-card p-5">
          <div className="flex min-w-0 items-center gap-3">
            <span className="grid size-11 shrink-0 place-items-center rounded-xl bg-[var(--brand-blue-soft)] text-[var(--star-strong)]">
              <Film className="size-5" />
            </span>
            <div className="space-y-1">
              <h2 className="text-sm font-medium">{t('workflows:sprite.title')}</h2>
              <p className="text-sm text-muted-foreground">{t('workflows:sprite.description')}</p>
            </div>
          </div>
          <Button variant="outline" onClick={openSpriteTemplate}>
            {t('workflows:sprite.openStudio')}
            <ArrowUpRight className="size-4" />
          </Button>
        </div>
      )}
      {view === 'workflows' ? (
        <WorkflowAssetList
          workflows={catalog.visibleWorkflows}
          runs={catalog.data.runs}
          busyId={catalog.busyId}
          language={language}
          t={t}
          onRun={setRunTarget}
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
  const [runDialogOpen, setRunDialogOpen] = useState(false)
  const [inspectorOpen, setInspectorOpen] = useState(false)
  const [wideEditor, setWideEditor] = useState(false)
  const editorElement = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const element = editorElement.current
    if (!element) return
    const measure = () => setWideEditor(element.clientWidth >= 1000)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [editor.loading])
  const openRunDialog = useCallback(() => {
    if (!busy && !running) setRunDialogOpen(true)
  }, [busy, running])

  usePagePrimaryAction(registerPrimaryAction, publishWorkflow)
  useEffect(
    () =>
      registerWorkflowActions?.({
        save: () => saveWorkflow('draft'),
        run: running ? stopWorkflow : openRunDialog,
        busy: busy || editor.loading || !editor.draft,
        running,
      }),
    [
      busy,
      editor.draft,
      editor.loading,
      registerWorkflowActions,
      openRunDialog,
      running,
      saveWorkflow,
      stopWorkflow,
    ],
  )

  if (editor.loading) {
    return <WorkflowLoading label={t('workflows:workflowsPage.loadingWorkflowEditor')} />
  }
  if (!editor.draft) {
    return (
      <AppEmptyState size="sm" className="gap-4 rounded-xl border bg-card p-6">
        <AlertTriangle className="size-6 text-destructive" aria-hidden="true" />
        <h2>{t('workflows:workflowsPage.workflowEditorLoadFailed')}</h2>
        <WorkflowError message={editor.error} />
        <div className="flex flex-wrap justify-center gap-2">
          <Button onClick={editor.retryLoad}>
            <RefreshCw className="size-4" />
            {t('workflows:workflowsPage.retryLoading')}
          </Button>
          <Button variant="outline" onClick={() => navigate(PAGE_PATHS.workflows)}>
            {t('workflows:workflowsPage.backToWorkflows')}
          </Button>
        </div>
      </AppEmptyState>
    )
  }

  const inspector = (
    <WorkflowNodeInspector
      draft={editor.draft}
      catalog={editor.catalog}
      selectedNode={editor.selectedNode}
      selectedEdge={editor.selectedEdge}
      currentRun={editor.currentRun}
      onRunImageNode={(nodeId, sourceRunId) => {
        void editor.runImageNode(nodeId, sourceRunId)
      }}
      imageRunBusy={editor.busy || editor.running}
      language={language}
      t={t}
      systemNotificationPermission={editor.systemNotificationPermission}
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
  )
  const inspectorTitle = editor.selectedEdge
    ? t('workflows:workflowsPage.selectedConnection')
    : t('workflows:editor.nodeProperties')

  return (
    <div
      ref={editorElement}
      className="workflow-editor-page @container/workflow flex min-w-0 flex-col gap-3"
    >
      {runDialogOpen && (
        <WorkflowRunDialog
          workflow={editor.draft}
          onClose={() => setRunDialogOpen(false)}
          onRun={runWorkflow}
        />
      )}
      <div className="flex min-w-0 flex-wrap items-center gap-2 rounded-xl border bg-card p-2">
        <nav aria-label={t('workflows:workflowsPage.editorNavigation')}>
          <Button variant="ghost" size="sm" onClick={() => navigate(PAGE_PATHS.workflows)}>
            <ArrowLeft />
            {t('workflows:workflowsPage.backToWorkflows')}
          </Button>
        </nav>
        <div className="hidden h-5 w-px bg-border @min-[640px]/workflow:block" />
        <p className="min-w-0 flex-1 truncate px-1 text-sm font-medium" title={editor.draft.name}>
          {editor.draft.name}
        </p>
        <div className="flex flex-wrap items-center gap-2 max-[480px]:w-full max-[480px]:justify-end">
          <WorkflowNodePalette
            nodeCount={editor.draft.nodes.length}
            t={t}
            onAddNode={editor.addNode}
          />
          <Dialog>
            <DialogTrigger asChild>
              <Button variant="ghost" size="sm">
                <Settings2 />
                {t('workflows:workflowsPage.workflowSettings')}
              </Button>
            </DialogTrigger>
            <DialogContent className="sm:max-w-xl">
              <DialogHeader>
                <DialogTitle>{t('workflows:workflowsPage.workflowSettings')}</DialogTitle>
                <DialogDescription>{t('workflows:editor.settingsDescription')}</DialogDescription>
              </DialogHeader>
              <WorkflowSettings
                draft={editor.draft}
                catalog={editor.catalog}
                t={t}
                onUpdateDraft={editor.updateDraft}
                onInputUploadBusy={editor.onInputUploadBusy}
              />
            </DialogContent>
          </Dialog>
          {!wideEditor && (
            <Button variant="ghost" size="sm" onClick={() => setInspectorOpen(true)}>
              <PanelRight />
              {t('workflows:editor.nodeProperties')}
            </Button>
          )}
        </div>
      </div>
      <WorkflowError message={editor.error} />
      {editor.running && editor.currentRun && (
        <WorkflowRunningNotice run={editor.currentRun} t={t} />
      )}
      <div
        className={cn(
          'builder-layout grid h-[max(440px,calc(100vh-220px))] min-h-[440px] min-w-0 overflow-hidden rounded-xl border bg-card supports-[height:100dvh]:h-[max(440px,calc(100dvh-220px))]',
          wideEditor ? 'grid-cols-[minmax(0,1fr)_320px]' : 'grid-cols-1',
        )}
      >
        <WorkflowEditorCanvas
          draft={editor.draft}
          selectedNodeId={editor.selectedNodeId}
          selectedEdgeId={editor.selectedEdgeId}
          t={t}
          onAddNode={editor.addNode}
          onConnect={editor.addEdge}
          onMoveNode={editor.moveNode}
          onSelectNode={(id) => {
            editor.selectNode(id)
            setInspectorOpen(true)
          }}
          onSelectEdge={(id) => {
            editor.selectEdge(id)
            setInspectorOpen(true)
          }}
          onClearSelection={editor.clearSelection}
          onDeleteNodes={editor.removeNodes}
          onDeleteEdges={editor.removeEdges}
        />
        {wideEditor && (
          <aside
            className="flex min-h-0 min-w-0 flex-col border-l bg-card"
            aria-label={inspectorTitle}
          >
            <header className="border-b px-4 py-3">
              <h2 className="text-sm font-medium">{inspectorTitle}</h2>
            </header>
            <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain p-4">{inspector}</div>
          </aside>
        )}
      </div>
      {!wideEditor && (
        <Sheet open={inspectorOpen} onOpenChange={setInspectorOpen}>
          <SheetContent className="gap-0 data-[side=right]:w-[min(400px,100vw)] data-[side=right]:sm:max-w-[400px]">
            <SheetHeader className="border-b pr-12">
              <SheetTitle>{inspectorTitle}</SheetTitle>
              <SheetDescription className="sr-only">
                {t('workflows:editor.inspectorDescription')}
              </SheetDescription>
            </SheetHeader>
            <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain p-4">{inspector}</div>
          </SheetContent>
        </Sheet>
      )}
    </div>
  )
}
