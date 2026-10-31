// 工作流：列表页（功能页模板，规格 v2 第 10 节 A6 第 3 批）+ 节点编辑器（路由 /workflows/:id，全宽例外）。
// 列表页的搜索、导入与“新建工作流”都在页面自己的工具行里；编辑器只有一条 52px 页头（‹ 工作流 · 标题 ·
// 添加节点 / 工作流设置 / 保存草稿 / 试运行 / 发布），下面画布与属性栏铺满剩余空间。
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import '@xyflow/react/dist/style.css'
import {
  AlertTriangle,
  ChevronLeft,
  FileUp,
  PanelRight,
  Play,
  Plus,
  RefreshCw,
  Rocket,
  Save,
  Settings2,
  Square,
} from 'lucide-react'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { FEATURE_SEARCH_INPUT_ID } from '@/app/more-pages'
import { getWorkflowEditorMeta } from '@/app/navigation'
import { PAGE_PATHS, workflowPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { FeaturePage, FeatureSearch, FeatureToolbar } from '@/components/app/FeaturePage'
import { PageFrame } from '@/components/app/PageFrame'
import { PageState } from '@/components/app/PageState'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
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
  // “‹ 功能”：从功能进入时由应用壳给出（规格 2.4）。
  onBack?: () => void
}

type WorkflowBuilderProps = {
  notify: Notify
  // 应用壳只用它判断“新建 / 编辑”标题（手机顶栏的页名）；按钮都在编辑器自己的页头里。
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

// 编辑器页头按钮：桌面 28px，手机与粗指针下撑到 44px 触控目标。
const TOOLBAR_BUTTON = 'max-[650px]:h-11 pointer-coarse:h-11'

export function WorkflowsPage({ notify, requestConfirm, onBack }: WorkflowsPageProps) {
  const { t, language } = useI18n()
  const navigate = useNavigate()
  const searchShortcut = useShortcutLabel('focusSearch')
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
  // 搜索词由页面自己持有（换页即清空，与原共享页头一致）。
  const [query, setQuery] = useState('')
  const catalog = useWorkflowCatalog({ notify, requestConfirm, query })
  const [runTarget, setRunTarget] = useState<Workflow | null>(null)
  const fileRef = useRef<HTMLInputElement>(null)
  const needle = query.trim().toLocaleLowerCase()
  const visibleRuns = needle
    ? catalog.data.runs.filter((run) =>
        `${run.workflowName || run.workflowId}`.toLocaleLowerCase().includes(needle),
      )
    : catalog.data.runs
  const title = t('navigation:navigation.workflows')

  return (
    <FeaturePage
      title={title}
      description={t('workflows:workflowsPage.lede')}
      back={
        onBack && {
          label: t('navigation:sidebar.features'),
          ariaLabel: t('navigation:shell.backToFeaturesLabel'),
          onBack,
        }
      }
      toolbar={
        <FeatureToolbar
          search={
            <FeatureSearch
              id={FEATURE_SEARCH_INPUT_ID}
              value={query}
              onValueChange={setQuery}
              placeholder={t('navigation:pageHeader.searchPage', { page: title })}
              shortcut={searchShortcut}
              clearLabel={t('navigation:more.clearSearch')}
            />
          }
          actions={
            <>
              <Button
                type="button"
                variant="secondary"
                className="h-9"
                onClick={() => fileRef.current?.click()}
              >
                <FileUp />
                {t('workflows:workflowsPage.import')}
              </Button>
              <input
                ref={fileRef}
                // 隐藏的文件选择框：由“导入”按钮代为点开，不占位也不进 Tab 顺序。
                hidden
                type="file"
                accept="application/zip,.zip"
                onChange={(event) => {
                  const file = event.target.files?.[0]
                  event.currentTarget.value = ''
                  if (file) void catalog.importWorkflow(file)
                }}
              />
              <Button type="button" className="h-9" onClick={() => navigate(workflowPath('new'))}>
                <Plus />
                {t('workflows:workflowsPage.newWorkflow')}
              </Button>
            </>
          }
          filters={
            <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-4 gap-y-2">
              <WorkflowViewTabs value={view} t={t} onChange={setView} />
              {!catalog.loading && <WorkflowOperationsSummary data={catalog.data} t={t} />}
            </div>
          }
        />
      }
    >
      {runTarget && (
        <WorkflowRunDialog
          workflow={runTarget}
          onClose={() => setRunTarget(null)}
          onRun={(inputs) => catalog.runWorkflow(runTarget, inputs)}
        />
      )}
      {catalog.loading ? (
        <PageState kind="loading" title={t('workflows:workflowsPage.loadingWorkflows')} />
      ) : catalog.error && !catalog.loaded ? (
        // 首次读取失败不显示成“还没有工作流”：写明错误并给重试（老问题，原来是空列表 + 顶部错误条）。
        <PageState
          kind="error"
          title={t('workflows:workflowsPage.loadFailed')}
          description={catalog.error}
          action={
            <Button
              variant="secondary"
              className="max-[650px]:h-11"
              onClick={() => void catalog.reload()}
            >
              <RefreshCw />
              {t('workflows:workflowsPage.retryLoading')}
            </Button>
          }
        />
      ) : (
        <>
          {catalog.error && (
            <div className="mt-4">
              <WorkflowError message={catalog.error} />
            </div>
          )}
          {view === 'workflows' ? (
            <WorkflowAssetList
              workflows={catalog.visibleWorkflows}
              runs={catalog.data.runs}
              busyId={catalog.busyId}
              query={query}
              language={language}
              t={t}
              onRun={setRunTarget}
              onEdit={(workflowId) => navigate(workflowPath(workflowId))}
              onDuplicate={(workflow) => void catalog.duplicateWorkflow(workflow)}
              onExport={(workflow) => void catalog.exportWorkflow(workflow)}
              onDelete={(workflow) => void catalog.removeWorkflow(workflow)}
            />
          ) : view === 'runs' ? (
            <WorkflowRunHistory
              runs={visibleRuns}
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
              query={query}
              t={t}
              onOpenTemplate={(templateId) =>
                navigate(`${workflowPath('new')}?template=${encodeURIComponent(templateId)}`)
              }
            />
          )}
        </>
      )}
    </FeaturePage>
  )
}

// 编辑器页头（规格 2.7“节点编辑器全宽例外”，只统一页头与返回）：一条 52px 顶栏，与会话页顶栏同一结构——
// 空白处是窗口拖拽区（控件不拖），Windows 自绘标题栏时右侧让出 144px；左侧“‹ 工作流”回列表，
// h1 是“新建 / 编辑工作流”（手机上页名在外壳顶栏，h1 只给读屏），后面是工作流名称；右侧是编辑工具与保存 / 试运行 / 发布。
function WorkflowEditorHeader({
  title,
  name,
  tools,
  commands,
  onBack,
  t,
}: {
  title: string
  name?: string
  tools?: ReactNode
  commands?: ReactNode
  onBack: () => void
  t: ReturnType<typeof useI18n>['t']
}) {
  const phone = usePhoneShell()
  return (
    <header
      data-workflow-editor-header
      data-focus-region="header"
      data-window-drag-region
      className="@container/workflow-header flex min-h-[52px] min-w-0 shrink-0 flex-wrap items-center gap-x-2 gap-y-1.5 border-b border-stroke-soft py-2 pr-3 pl-3 [[data-custom-titlebar]_&]:pr-[144px] max-[650px]:pb-2.5 max-[650px]:pt-0"
    >
      <div
        className="flex min-w-0 flex-1 basis-56 items-center gap-1 max-[650px]:basis-0"
        data-window-drag-region
      >
        <nav aria-label={t('workflows:workflowsPage.editorNavigation')} className="flex-none">
          <button
            type="button"
            data-back-to-workflows
            aria-label={t('workflows:workflowsPage.backToWorkflows')}
            onClick={onBack}
            className="inline-flex h-7 items-center gap-0.5 rounded-[var(--r-sm)] pr-1.5 pl-0.5 text-[13px] text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus max-[650px]:-ml-1.5 max-[650px]:h-11 max-[650px]:min-w-11 max-[650px]:pr-2 max-[650px]:text-sm max-[650px]:text-content-secondary pointer-coarse:h-11"
          >
            <ChevronLeft className="size-4 max-[650px]:size-5" aria-hidden="true" />
            <span>{t('workflows:workflowsPage.workflows')}</span>
          </button>
        </nav>
        <span aria-hidden="true" className={cn('text-content-faint', phone && 'hidden')}>
          /
        </span>
        <h1
          className={cn(
            'flex-none text-[13px] leading-5 font-medium text-content',
            phone && 'sr-only',
          )}
        >
          {title}
        </h1>
        {name && (
          <p
            data-workflow-name
            className="min-w-0 truncate pl-1 text-[13px] text-content-muted max-[650px]:text-sm max-[650px]:text-content"
            title={name}
          >
            {name}
          </p>
        )}
      </div>
      {tools && (
        <div className="flex flex-none items-center gap-1 max-[650px]:[&_button]:h-11 max-[650px]:[&_button]:min-w-11 max-[650px]:[&_button]:px-2.5 pointer-coarse:[&_button]:h-11">
          {tools}
        </div>
      )}
      {commands && (
        <>
          {tools && (
            <span aria-hidden="true" className="mx-0.5 h-4 w-px bg-stroke max-[650px]:hidden" />
          )}
          <div className="flex flex-none items-center gap-1.5 max-[650px]:grid max-[650px]:w-full max-[650px]:grid-cols-3 max-[650px]:[&_button]:h-11 pointer-coarse:[&_button]:h-11">
            {commands}
          </div>
        </>
      )}
    </header>
  )
}

// 编辑器工具的文字：桌面写全，手机只留图标（读屏名称不变，触控目标 44）。
function ToolLabel({ children }: { children: ReactNode }) {
  return <span className="max-[650px]:sr-only">{children}</span>
}

export function WorkflowBuilder({ notify, registerWorkflowActions }: WorkflowBuilderProps) {
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

  // 草稿有 id 才是已保存的工作流；加载前按路由推断，找不到的 id 会落到空白草稿，仍算新建。
  const editing = editor.draft ? Boolean(editor.draft.id) : workflowId !== 'new'
  useEffect(() => registerWorkflowActions?.({ editing }), [editing, registerWorkflowActions])
  const title = getWorkflowEditorMeta(t, editing)[0]
  const backToList = () => navigate(PAGE_PATHS.workflows)

  // 只在运行进行中把本次运行的节点状态投到画布上；运行结束后画布回到纯编辑视图。
  const runningNodes = editor.running ? editor.currentRun?.nodes : undefined
  const runningNodeStatus = useMemo(
    () => (runningNodes ? new Map(runningNodes.map((node) => [node.id, node.status])) : undefined),
    [runningNodes],
  )

  // 读取中与读取失败也保留页头（返回与标题），页面不跳。
  if (editor.loading || !editor.draft) {
    return (
      <PageFrame variant="workbench">
        <div className="workflow-editor-page flex h-full min-h-0 min-w-0 flex-col">
          <WorkflowEditorHeader title={title} onBack={backToList} t={t} />
          {editor.loading ? (
            <PageState kind="loading" title={t('workflows:workflowsPage.loadingWorkflowEditor')} />
          ) : (
            <PageState
              kind="error"
              title={t('workflows:workflowsPage.workflowEditorLoadFailed')}
              description={editor.error}
              action={
                <div className="flex flex-wrap justify-center gap-2 max-[650px]:[&>button]:h-11">
                  <Button onClick={editor.retryLoad}>
                    <RefreshCw className="size-4" />
                    {t('workflows:workflowsPage.retryLoading')}
                  </Button>
                  <Button variant="outline" onClick={backToList}>
                    {t('workflows:workflowsPage.backToWorkflows')}
                  </Button>
                </div>
              }
            />
          )}
        </div>
      </PageFrame>
    )
  }
  const draft = editor.draft

  const inspector = (
    <WorkflowNodeInspector
      draft={draft}
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
      {/* 编辑器铺满主纸面：52px 页头与运行条以细线分隔，画布与右侧属性栏占满剩余高度。 */}
      <div
        ref={editorElement}
        className="workflow-editor-page @container/workflow flex h-full min-h-0 min-w-0 flex-col"
      >
        {runDialogOpen && (
          <WorkflowRunDialog
            workflow={draft}
            onClose={() => setRunDialogOpen(false)}
            onRun={runWorkflow}
          />
        )}
        <WorkflowEditorHeader
          title={title}
          name={draft.name}
          onBack={backToList}
          t={t}
          tools={
            <>
              <WorkflowNodePalette
                nodeCount={draft.nodes.length}
                t={t}
                onAddNode={editor.addNode}
              />
              <Dialog>
                <DialogTrigger asChild>
                  <Button
                    variant="ghost"
                    size="sm"
                    className={TOOLBAR_BUTTON}
                    title={t('workflows:workflowsPage.workflowSettings')}
                  >
                    <Settings2 />
                    <ToolLabel>{t('workflows:workflowsPage.workflowSettings')}</ToolLabel>
                  </Button>
                </DialogTrigger>
                <DialogContent className="sm:max-w-xl">
                  <DialogHeader>
                    <DialogTitle>{t('workflows:workflowsPage.workflowSettings')}</DialogTitle>
                    <DialogDescription>
                      {t('workflows:editor.settingsDescription')}
                    </DialogDescription>
                  </DialogHeader>
                  <WorkflowSettings
                    draft={draft}
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
                  title={t('workflows:editor.nodeProperties')}
                  onClick={() => setInspectorOpen(true)}
                >
                  <PanelRight />
                  <ToolLabel>{t('workflows:editor.nodeProperties')}</ToolLabel>
                </Button>
              )}
            </>
          }
          commands={
            <>
              <Button
                variant="outline"
                size="sm"
                disabled={busy || running}
                onClick={() => void saveWorkflow('draft')}
              >
                <Save />
                {t('workflows:editor.saveDraft')}
              </Button>
              <Button
                size="sm"
                disabled={busy}
                onClick={() => (running ? void stopWorkflow() : openRunDialog())}
              >
                {running ? <Square /> : <Play />}
                {running ? t('workflows:workflowsPage.stop') : t('workflows:editor.testRun')}
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={busy}
                onClick={() => void publishWorkflow()}
              >
                <Rocket />
                {t('workflows:editor.publish')}
              </Button>
            </>
          }
        />
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
              draft={draft}
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
