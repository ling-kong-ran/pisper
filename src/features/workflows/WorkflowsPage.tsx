import { PageFrame } from '@/components/app/PageFrame'
import { PageState } from '@/components/app/PageState'
import { PageToolbar } from '@/components/app/PageToolbar'
// 工作流页面：列表视图 + 编辑器（路由 /workflows/:id）的宿主，
// 管理工作流目录、保存与运行，并向壳层注册主操作。
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import '@xyflow/react/dist/style.css'
import { AlertTriangle, ArrowLeft, PanelRight, RefreshCw, Settings2 } from 'lucide-react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { PAGE_PATHS, workflowPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { cn } from '@/lib/utils'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { WorkflowActions } from '@/types/workflow'
import { WorkflowEditorCanvas, WorkflowNodePalette } from './WorkflowEditorCanvas'
import {
  WorkflowAssetList,
  WorkflowOperationsSummary,
  WorkflowRunHistory,
  WorkflowTemplateGallery,
  WorkflowViewTabs,
  type WorkflowView,
} from './WorkflowListSidebar'
import { WorkflowNodeInspector, WorkflowSettings } from './WorkflowNodeInspector'
import { WorkflowRunningNotice } from './WorkflowRunControls'
import { useWorkflowCatalog } from './useWorkflowCatalog'
import { useWorkflowEditor } from './useWorkflowEditor'
import { WorkflowRunDialog } from './WorkflowRunDialog'
import type { Workflow } from './types'

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
  registerWorkflowActions?: (actions: WorkflowActions) => () => void
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

// 加载态沿用页面状态原语：品牌轨道指示 + 标题，不再自绘卡片。
function WorkflowLoading({ label }: { label: string }) {
  return <PageState kind="loading" title={label} />
}

// 编辑器工具条按钮：桌面 28px，手机与粗指针下撑到 44px 触控目标。
const TOOLBAR_BUTTON = 'max-[650px]:h-11 pointer-coarse:h-11'

export function WorkflowsPage({ notify, requestConfirm, query = '' }: WorkflowsPageProps) {
  const { t, language } = useI18n()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const requestedView = searchParams.get('view')
  const view: WorkflowView =
    requestedView === 'runs' || requestedView === 'templates' ? requestedView : 'workflows'
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
    <PageFrame variant="collection">
      <div className="flex min-w-0 flex-col gap-3">
        {runTarget && (
          <WorkflowRunDialog
            workflow={runTarget}
            onClose={() => setRunTarget(null)}
            onRun={(inputs) => catalog.runWorkflow(runTarget, inputs)}
          />
        )}
        <WorkflowError message={catalog.error} />
        <PageToolbar>
          <WorkflowViewTabs value={view} t={t} onChange={setView} />
          <WorkflowOperationsSummary data={catalog.data} t={t} />
        </PageToolbar>
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
    </PageFrame>
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
  // null = 尚未测量：画布等到确定是否并排属性栏后才挂载，首个 fitView 才会按真实画布宽度居中。
  const [wideEditor, setWideEditor] = useState<boolean | null>(null)
  const editorElement = useRef<HTMLDivElement>(null)
  // 布局测量放在绘制前：测量后同步重渲染，用户看不到未测量的空画布帧。
  useLayoutEffect(() => {
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
        // 草稿有 id 才是已保存的工作流；加载前按路由推断，找不到的 id 会落到空白草稿，仍算新建。
        editing: editor.draft ? Boolean(editor.draft.id) : workflowId !== 'new',
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
      workflowId,
    ],
  )

  // 只在运行进行中把本次运行的节点状态投到画布上；运行结束后画布回到纯编辑视图。
  const runningNodes = editor.running ? editor.currentRun?.nodes : undefined
  const runningNodeStatus = useMemo(
    () => (runningNodes ? new Map(runningNodes.map((node) => [node.id, node.status])) : undefined),
    [runningNodes],
  )

  if (editor.loading) {
    return <WorkflowLoading label={t('workflows:workflowsPage.loadingWorkflowEditor')} />
  }
  if (!editor.draft) {
    return (
      <PageState
        kind="error"
        title={t('workflows:workflowsPage.workflowEditorLoadFailed')}
        description={editor.error}
        action={
          <div className="flex flex-wrap justify-center gap-2">
            <Button onClick={editor.retryLoad}>
              <RefreshCw className="size-4" />
              {t('workflows:workflowsPage.retryLoading')}
            </Button>
            <Button variant="outline" onClick={() => navigate(PAGE_PATHS.workflows)}>
              {t('workflows:workflowsPage.backToWorkflows')}
            </Button>
          </div>
        }
      />
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
    <PageFrame variant="workbench">
      {/* 编辑器铺满主纸面：顶部 44px 工具条与运行条以细线分隔，画布与右侧属性栏占满剩余高度。 */}
      <div
        ref={editorElement}
        className="workflow-editor-page @container/workflow flex h-full min-h-0 min-w-0 flex-col"
      >
        {runDialogOpen && (
          <WorkflowRunDialog
            workflow={editor.draft}
            onClose={() => setRunDialogOpen(false)}
            onRun={runWorkflow}
          />
        )}
        <div className="flex min-h-11 min-w-0 shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b border-stroke-soft px-2 py-1.5">
          <nav aria-label={t('workflows:workflowsPage.editorNavigation')}>
            <Button
              variant="ghost"
              size="sm"
              className={TOOLBAR_BUTTON}
              onClick={() => navigate(PAGE_PATHS.workflows)}
            >
              <ArrowLeft />
              {t('workflows:workflowsPage.backToWorkflows')}
            </Button>
          </nav>
          <span
            aria-hidden="true"
            className="hidden h-4 w-px bg-stroke @min-[640px]/workflow:block"
          />
          <p className="min-w-0 flex-1 truncate px-1 text-sm font-medium" title={editor.draft.name}>
            {editor.draft.name}
          </p>
          <div className="flex flex-wrap items-center gap-1 max-[480px]:w-full max-[480px]:justify-end">
            <WorkflowNodePalette
              nodeCount={editor.draft.nodes.length}
              t={t}
              onAddNode={editor.addNode}
            />
            <Dialog>
              <DialogTrigger asChild>
                <Button variant="ghost" size="sm" className={TOOLBAR_BUTTON}>
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
            {wideEditor === false && (
              <Button
                variant="ghost"
                size="sm"
                className={TOOLBAR_BUTTON}
                onClick={() => setInspectorOpen(true)}
              >
                <PanelRight />
                {t('workflows:editor.nodeProperties')}
              </Button>
            )}
          </div>
        </div>
        {editor.error && (
          <div className="shrink-0 px-3 pt-3">
            <WorkflowError message={editor.error} />
          </div>
        )}
        {editor.running && editor.currentRun && (
          <WorkflowRunningNotice run={editor.currentRun} t={t} />
        )}
        <div
          className={cn(
            'builder-layout grid min-h-0 min-w-0 flex-1 overflow-hidden',
            wideEditor ? 'grid-cols-[minmax(0,1fr)_320px]' : 'grid-cols-1',
          )}
        >
          {wideEditor !== null && (
            <WorkflowEditorCanvas
              draft={editor.draft}
              selectedNodeId={editor.selectedNodeId}
              selectedEdgeId={editor.selectedEdgeId}
              nodeStatus={runningNodeStatus}
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
          )}
          {wideEditor && (
            <aside
              className="flex min-h-0 min-w-0 flex-col border-l border-stroke-soft bg-panel"
              aria-label={inspectorTitle}
            >
              <header className="flex h-11 shrink-0 items-center border-b border-stroke-soft px-4">
                <h2 className="text-sm font-medium">{inspectorTitle}</h2>
              </header>
              <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain p-4">
                {inspector}
              </div>
            </aside>
          )}
        </div>
        {wideEditor === false && (
          <Sheet open={inspectorOpen} onOpenChange={setInspectorOpen}>
            <SheetContent className="gap-0 data-[side=right]:w-[min(400px,100vw)] data-[side=right]:sm:max-w-[400px]">
              <SheetHeader className="border-b border-stroke-soft pr-12">
                <SheetTitle>{inspectorTitle}</SheetTitle>
                <SheetDescription className="sr-only">
                  {t('workflows:editor.inspectorDescription')}
                </SheetDescription>
              </SheetHeader>
              <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain p-4">
                {inspector}
              </div>
            </SheetContent>
          </Sheet>
        )}
      </div>
    </PageFrame>
  )
}
