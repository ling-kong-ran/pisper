import { workflowErrorMessage } from './useWorkflowCatalog'
// 工作流列表：工作流资产表、运行记录日志与模板库，按 Quiet Precision 的细线卡片组织。
import { useRef, useState } from 'react'
import {
  Check,
  ChevronRight,
  CircleX,
  Copy,
  Download,
  FileUp,
  MoreHorizontal,
  Pencil,
  Play,
  RefreshCw,
  Square,
  Trash2,
} from 'lucide-react'
import { InlineEmptyState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Progress } from '@/components/ui/progress'
import { OrbitSpinner } from '@/components/ui/spinner'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { relativeTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { Workflow, WorkflowRun, WorkflowsData } from './types'
import {
  templateDescription,
  templateName,
  WORKFLOW_TEMPLATES,
  type WorkflowTranslate,
} from './workflow-templates'
import { RunStatusBadge, RunStatusSquare, RunStepMeter } from './WorkflowRunStatus'

export type WorkflowView = 'workflows' | 'runs' | 'templates'

// 时长用紧凑的拉丁单位（42s / 1m 36s），配合等宽数字在列表里对齐、计时时不跳动。
function durationLabel(durationMs?: number) {
  const seconds = Math.max(0, Math.round((Number(durationMs) || 0) / 1000))
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`
}

function statusLabel(status: string, t: WorkflowTranslate) {
  if (status === 'running') return t('workflows:workflowsPage.running')
  if (status === 'waiting_approval') return t('workflows:workflowsPage.waitingApproval')
  if (status === 'completed') return t('workflows:workflowsPage.completed')
  if (status === 'failed') return t('workflows:workflowsPage.failed')
  if (status === 'cancelled') return t('workflows:workflowsPage.stopped')
  if (status === 'interrupted') return t('workflows:workflowsPage.interrupted')
  return t('workflows:workflowsPage.draft')
}

// 卡片头：次级表面上的 13/500 标题 + 等宽计数，与设置页的列表卡片同一结构。
const CARD = 'overflow-hidden rounded-md border border-stroke bg-panel'
const CARD_HEAD =
  'flex min-h-11 min-w-0 items-center justify-between gap-3 border-b border-stroke-soft bg-surface-subtle px-3.5 py-1.5'
// 行内图标键：桌面 28px，手机与粗指针下撑到 44px 触控目标。
const ROW_ICON_BUTTON = 'max-[650px]:size-11 pointer-coarse:size-11'

export function WorkflowViewTabs({
  value,
  onChange,
  t,
}: {
  value: WorkflowView
  onChange: (value: WorkflowView) => void
  t: WorkflowTranslate
}) {
  // 分段控件：凹陷轨道 + 选中键帽；手机上铺满整行，每段至少 44px 高。
  const trigger = 'min-w-14 flex-none px-3 max-[650px]:flex-1'
  return (
    <Tabs
      value={value}
      onValueChange={(next) => onChange(next as WorkflowView)}
      className="max-[650px]:w-full"
    >
      <TabsList className="max-w-full justify-start overflow-x-auto overflow-y-hidden [scrollbar-width:none] max-[650px]:min-h-11 max-[650px]:w-full pointer-coarse:min-h-11 [&::-webkit-scrollbar]:hidden">
        <TabsTrigger className={trigger} value="workflows">
          {t('workflows:workflowsPage.workflows')}
        </TabsTrigger>
        <TabsTrigger className={trigger} value="runs">
          {t('workflows:workflowsPage.runHistory')}
        </TabsTrigger>
        <TabsTrigger className={trigger} value="templates">
          {t('workflows:workflowsPage.templates')}
        </TabsTrigger>
      </TabsList>
    </Tabs>
  )
}

export function WorkflowAssetList({
  workflows,
  runs,
  busyId,
  language,
  t,
  onRun,
  onEdit,
  onDuplicate,
  onExport,
  onImport,
  onDelete,
}: {
  workflows: Workflow[]
  runs: WorkflowRun[]
  busyId: string
  language: string
  t: WorkflowTranslate
  onRun: (workflow: Workflow) => void
  onEdit: (workflowId: string) => void
  onDuplicate: (workflow: Workflow) => void
  onExport: (workflow: Workflow) => void
  onImport: (value: File) => void
  onDelete: (workflow: Workflow) => void
}) {
  const fileRef = useRef<HTMLInputElement>(null)
  const latestRun = (workflowId: string) =>
    runs
      .filter((run) => run.workflowId === workflowId)
      .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))[0]

  return (
    <section className={cn('workflow-assets-panel', CARD)}>
      <header className={cn('workflow-table-head', CARD_HEAD)}>
        <h2 className="flex min-w-0 items-baseline gap-2 text-sm font-medium text-content">
          {t('workflows:workflowsPage.workflows')}
          <span className="font-normal text-content-muted tabular-nums">{workflows.length}</span>
        </h2>
        <Button
          size="sm"
          variant="secondary"
          className="max-[650px]:h-11"
          onClick={() => fileRef.current?.click()}
        >
          <FileUp data-icon="inline-start" />
          {t('workflows:workflowsPage.import')}
        </Button>
        <input
          ref={fileRef}
          className="sr-only"
          type="file"
          accept="application/zip,.zip"
          onChange={(event) => {
            const file = event.target.files?.[0]
            event.currentTarget.value = ''
            if (!file) return
            onImport(file)
          }}
        />
      </header>
      {workflows.length ? (
        <ul className="divide-y divide-stroke-soft">
          {workflows.map((workflow) => {
            const run = latestRun(workflow.id)
            const running = ['running', 'waiting_approval'].includes(run?.status || '')
            const runBusy = running || busyId === workflow.id
            return (
              <li
                className="workflow-asset-row grid min-h-[52px] grid-cols-[minmax(0,1fr)_minmax(110px,150px)_minmax(150px,210px)_auto] items-center gap-4 px-3.5 py-2 transition-colors duration-(--d-fast) hover:bg-surface-subtle max-[650px]:grid-cols-[minmax(0,1fr)_auto] max-[650px]:gap-x-2 max-[650px]:gap-y-1.5 max-[650px]:py-2.5 max-[650px]:pr-2"
                key={workflow.id}
              >
                <button
                  type="button"
                  className="workflow-asset-main flex min-w-0 flex-col gap-0.5 text-left focus-visible:-outline-offset-2"
                  onClick={() => onEdit(workflow.id)}
                >
                  <strong className="truncate text-sm font-medium text-content max-[650px]:text-[15px]">
                    {workflow.name}
                  </strong>
                  <small
                    className={cn(
                      'truncate text-xs text-content-muted',
                      !workflow.description && 'font-mono',
                    )}
                  >
                    {workflow.description || workflow.cwd}
                  </small>
                </button>
                {/* 桌面上两列状态各占一格；手机上合成主标题下方的一行徽标。 */}
                <div className="contents max-[650px]:col-span-2 max-[650px]:flex max-[650px]:min-w-0 max-[650px]:flex-wrap max-[650px]:items-center max-[650px]:gap-x-2 max-[650px]:gap-y-1">
                  <span className="workflow-state flex min-w-0 items-center gap-2">
                    <RunStatusBadge
                      status={workflow.status === 'published' ? 'completed' : 'draft'}
                      label={
                        workflow.status === 'published'
                          ? t('workflows:workflowsPage.published')
                          : t('workflows:workflowsPage.draft')
                      }
                    />
                    <span className="font-mono text-xs text-content-muted tabular-nums">
                      v{workflow.revision}
                    </span>
                  </span>
                  <span className="workflow-state flex min-w-0 items-center gap-2">
                    {run ? (
                      <RunStatusBadge status={run.status} label={statusLabel(run.status, t)} />
                    ) : (
                      <span className="text-xs text-content-muted">
                        {t('workflows:workflowsPage.neverRun')}
                      </span>
                    )}
                    <span className="truncate text-xs text-content-muted tabular-nums">
                      {run
                        ? relativeTime(run.startedAt, language)
                        : workflow.visibility === 'shared'
                          ? t('workflows:workflowsPage.shared')
                          : t('workflows:workflowsPage.private')}
                    </span>
                  </span>
                </div>
                <div className="workflow-row-actions flex items-center justify-end gap-0.5 max-[650px]:col-start-2 max-[650px]:row-start-1">
                  <Button
                    size="icon-sm"
                    variant="outline"
                    className={ROW_ICON_BUTTON}
                    disabled={runBusy}
                    title={t('workflows:workflowsPage.run')}
                    aria-label={`${t('workflows:workflowsPage.run')}: ${workflow.name}`}
                    onClick={() => onRun(workflow)}
                  >
                    {busyId === workflow.id ? <RefreshCw className="animate-spin" /> : <Play />}
                  </Button>
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    className={ROW_ICON_BUTTON}
                    title={t('workflows:workflowsPage.edit')}
                    aria-label={`${t('workflows:workflowsPage.edit')}: ${workflow.name}`}
                    onClick={() => onEdit(workflow.id)}
                  >
                    <Pencil />
                  </Button>
                  {/* 桌面直接给出全部行操作；手机保留运行与编辑，其余收进 44px 的「更多」菜单。 */}
                  <span className="flex items-center gap-0.5 max-[650px]:hidden">
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      className={ROW_ICON_BUTTON}
                      title={t('workflows:workflowsPage.duplicateWorkflow')}
                      aria-label={`${t('workflows:workflowsPage.duplicateWorkflow')}: ${workflow.name}`}
                      onClick={() => onDuplicate(workflow)}
                    >
                      <Copy />
                    </Button>
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      className={ROW_ICON_BUTTON}
                      title={t('workflows:workflowsPage.export')}
                      aria-label={`${t('workflows:workflowsPage.export')}: ${workflow.name}`}
                      onClick={() => onExport(workflow)}
                    >
                      <Download />
                    </Button>
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      className={cn(ROW_ICON_BUTTON, 'hover:bg-danger-soft hover:text-danger')}
                      disabled={running}
                      title={t('workflows:workflowsPage.delete')}
                      aria-label={`${t('workflows:workflowsPage.delete')}: ${workflow.name}`}
                      onClick={() => onDelete(workflow)}
                    >
                      <Trash2 />
                    </Button>
                  </span>
                  <span className="hidden max-[650px]:contents">
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button
                          size="icon"
                          variant="ghost"
                          className="size-11 text-content-muted"
                          aria-label={t('workflows:workflowsPage.moreActionsFor', {
                            name: workflow.name,
                          })}
                        >
                          <MoreHorizontal />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end" className="min-w-44">
                        <DropdownMenuItem
                          className="min-h-11"
                          onSelect={() => onDuplicate(workflow)}
                        >
                          <Copy />
                          {t('workflows:workflowsPage.duplicateWorkflow')}
                        </DropdownMenuItem>
                        <DropdownMenuItem className="min-h-11" onSelect={() => onExport(workflow)}>
                          <Download />
                          {t('workflows:workflowsPage.export')}
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem
                          className="min-h-11"
                          variant="destructive"
                          disabled={running}
                          onSelect={() => onDelete(workflow)}
                        >
                          <Trash2 />
                          {t('workflows:workflowsPage.delete')}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </span>
                </div>
              </li>
            )
          })}
        </ul>
      ) : (
        <InlineEmptyState
          compact
          title={t('workflows:workflowsPage.noMatchingWorkflows')}
          description={t('workflows:workflowsPage.emptyHint')}
          className="min-h-[150px]"
        />
      )}
    </section>
  )
}

export function WorkflowRunHistory({
  runs,
  busyId,
  language,
  t,
  onStop,
  onRetry,
  onApproval,
}: {
  runs: WorkflowRun[]
  busyId: string
  language: string
  t: WorkflowTranslate
  onStop: (run: WorkflowRun) => void
  onRetry: (run: WorkflowRun) => void
  onApproval: (run: WorkflowRun, nodeId: string, approved: boolean) => void
}) {
  const [expandedId, setExpandedId] = useState(runs[0]?.id || '')
  const ordered = [...runs].sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))
  if (!ordered.length) return <InlineEmptyState title={t('workflows:workflowsPage.noRunHistory')} />
  // 与工具活动日志同一语言：一张细线卡片，每次运行一行；展开后在次级表面里逐个列出节点。
  return (
    <ul className={cn('divide-y divide-stroke-soft', CARD)}>
      {ordered.map((run) => {
        const expanded = expandedId === run.id
        const progress = Math.round(
          ((Number(run.completedNodes) || 0) / Math.max(1, Number(run.totalNodes) || 1)) * 100,
        )
        return (
          <li key={run.id}>
            <button
              type="button"
              aria-expanded={expanded}
              className="workflow-run-summary grid min-h-[52px] w-full grid-cols-[14px_18px_minmax(0,1fr)_minmax(64px,160px)_auto] items-center gap-2.5 px-3.5 py-2 text-left transition-colors duration-(--d-fast) hover:bg-surface-subtle focus-visible:-outline-offset-2 aria-expanded:bg-surface-subtle max-[650px]:min-h-14 max-[650px]:grid-cols-[14px_18px_minmax(0,1fr)_auto] max-[650px]:pl-3"
              onClick={() => setExpandedId(expanded ? '' : run.id)}
            >
              <ChevronRight
                aria-hidden="true"
                className={cn(
                  'size-3.5 text-content-muted transition-transform duration-(--d-fast) ease-precise',
                  expanded && 'rotate-90',
                )}
              />
              <RunStatusSquare status={run.status} />
              <span className="flex min-w-0 flex-col gap-0.5">
                <strong className="truncate text-sm font-medium text-content">
                  {run.workflowName || run.workflowId}
                </strong>
                <small className="truncate text-xs text-content-muted tabular-nums">
                  {statusLabel(run.status, t)} · {relativeTime(run.startedAt, language)} ·{' '}
                  <span className="font-mono">v{run.workflowRevision || 1}</span>
                </small>
              </span>
              <span className="flex min-w-0 items-center gap-2 max-[650px]:hidden">
                {run.nodes?.length ? (
                  <RunStepMeter nodes={run.nodes} className="flex-1" />
                ) : (
                  <Progress
                    value={run.status === 'completed' ? 100 : progress}
                    className="h-1 flex-1"
                  />
                )}
                <span className="shrink-0 font-mono text-xs text-content-muted tabular-nums">
                  {Number(run.completedNodes) || 0}/{Number(run.totalNodes) || 0}
                </span>
              </span>
              <em className="text-right font-mono text-xs text-content-muted not-italic tabular-nums">
                {durationLabel(run.durationMs)}
              </em>
            </button>
            {expanded && (
              <div className="workflow-run-detail flex flex-col gap-2.5 border-t border-stroke-soft bg-surface-subtle py-3 pr-3.5 pl-[50px] max-[650px]:pl-3">
                {(run.summary || run.error) && (
                  <p
                    className={cn(
                      'workflow-run-result flex items-start gap-2 text-xs leading-relaxed whitespace-pre-wrap',
                      run.error ? 'failed text-danger' : 'text-content-secondary',
                    )}
                  >
                    {run.error && (
                      <i
                        aria-hidden="true"
                        className="mt-[5px] size-1.5 shrink-0 rounded-full bg-danger"
                      />
                    )}
                    {run.error ? workflowErrorMessage(run.error) : run.summary}
                  </p>
                )}
                <ul className="divide-y divide-stroke-soft overflow-hidden rounded-md border border-stroke bg-panel">
                  {(run.nodes || []).map((node) => (
                    <li
                      className={`workflow-run-node grid min-h-[34px] grid-cols-[18px_minmax(0,1fr)_auto] items-center gap-2.5 px-3 py-1.5 max-[650px]:min-h-11 ${node.status}`}
                      key={node.id}
                    >
                      <RunStatusSquare status={node.status} />
                      <span className="flex min-w-0 flex-col gap-0.5">
                        <span className="flex min-w-0 items-baseline gap-2">
                          <strong className="truncate text-sm font-medium text-content">
                            {node.label}
                          </strong>
                          <small className="shrink-0 text-xs text-content-muted tabular-nums">
                            {t('workflows:workflowsPage.countAttempts', {
                              count: node.attempts || 0,
                            })}
                          </small>
                        </span>
                        {(node.error || node.summary) && (
                          <p
                            className={cn(
                              'text-xs leading-relaxed whitespace-pre-wrap',
                              node.error ? 'text-danger' : 'text-content-muted',
                            )}
                          >
                            {node.error ? workflowErrorMessage(node.error) : node.summary}
                          </p>
                        )}
                      </span>
                      {node.status === 'waiting_approval' ? (
                        <div className="workflow-approval-actions flex gap-1.5">
                          <Button
                            size="xs"
                            className="max-[650px]:h-11"
                            disabled={busyId === node.id}
                            onClick={() => onApproval(run, node.id, true)}
                          >
                            <Check data-icon="inline-start" />
                            {t('workflows:workflowsPage.approve')}
                          </Button>
                          <Button
                            size="xs"
                            variant="destructive"
                            className="max-[650px]:h-11"
                            disabled={busyId === node.id}
                            onClick={() => onApproval(run, node.id, false)}
                          >
                            <CircleX data-icon="inline-start" />
                            {t('workflows:workflowsPage.reject')}
                          </Button>
                        </div>
                      ) : (
                        <span className="font-mono text-xs text-content-muted tabular-nums">
                          {node.status === 'running' ? (
                            <OrbitSpinner className="size-3" />
                          ) : (
                            durationLabel(node.durationMs)
                          )}
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
                <div className="workflow-run-detail-actions flex gap-1.5 empty:hidden max-[650px]:[&>button]:h-11">
                  {['running', 'waiting_approval'].includes(run.status) ? (
                    <Button size="sm" variant="destructive" onClick={() => onStop(run)}>
                      <Square data-icon="inline-start" />
                      {t('workflows:workflowsPage.stop')}
                    </Button>
                  ) : ['failed', 'cancelled', 'interrupted'].includes(run.status) ? (
                    <Button size="sm" variant="outline" onClick={() => onRetry(run)}>
                      <RefreshCw data-icon="inline-start" />
                      {t('workflows:workflowsPage.retryRun')}
                    </Button>
                  ) : null}
                </div>
              </div>
            )}
          </li>
        )
      })}
    </ul>
  )
}

export function WorkflowTemplateGallery({
  t,
  onOpenTemplate,
}: {
  t: WorkflowTranslate
  onOpenTemplate: (templateId: string) => void
}) {
  return (
    <div className="workflow-template-gallery grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-2">
      {WORKFLOW_TEMPLATES.map((template) => {
        const Icon = template.Icon
        return (
          <button
            type="button"
            key={template.id}
            className="group grid min-h-[68px] grid-cols-[32px_minmax(0,1fr)_16px] items-center gap-3 rounded-md border border-stroke bg-panel p-3 text-left transition-colors duration-(--d-fast) hover:border-stroke-strong hover:bg-surface-subtle"
            onClick={() => onOpenTemplate(template.id)}
          >
            {/* 模板图标是一枚中性键帽，强调色留给选中与主操作。 */}
            <span className="grid size-8 place-items-center rounded-sm bg-key text-content-secondary shadow-key ring-1 ring-key-border">
              <Icon className="size-4" aria-hidden="true" />
            </span>
            <span className="flex min-w-0 flex-col gap-0.5">
              <strong className="truncate text-sm font-medium text-content">
                {templateName(template.id, t)}
              </strong>
              <small className="truncate text-xs text-content-muted">
                {templateDescription(template.id, t)}
              </small>
            </span>
            <ChevronRight
              aria-hidden="true"
              className="size-4 text-content-faint transition-colors group-hover:text-content-muted"
            />
          </button>
        )
      })}
    </div>
  )
}

export function WorkflowOperationsSummary({
  data,
  t,
}: {
  data: WorkflowsData
  t: WorkflowTranslate
}) {
  const failed = data.runs.filter((run) => run.status === 'failed').length
  const approvals = data.runs.filter((run) => run.status === 'waiting_approval').length
  const completed = data.runs.filter((run) => run.status === 'completed')
  const average = completed.length
    ? completed.reduce((sum, run) => sum + (run.durationMs || 0), 0) / completed.length
    : 0
  const stat = 'inline-flex items-baseline gap-1.5 whitespace-nowrap'
  const value = 'text-sm font-medium text-content tabular-nums'
  // 运营统计：数字 13/500 等宽、标签 12 弱化；只有确有运行时才出现轨道指示。
  return (
    <div className="workflow-operations-strip flex min-w-0 flex-wrap items-center gap-x-5 gap-y-1 text-xs text-content-muted max-[650px]:w-full max-[650px]:justify-between max-[650px]:gap-x-3">
      <span className={stat}>
        {data.limits.running > 0 && <OrbitSpinner className="size-3 self-center" />}
        <strong className={value}>{data.limits.running}</strong>
        {t('workflows:workflowsPage.running')}
      </span>
      <span className={stat}>
        <strong className={cn(value, approvals > 0 && 'text-warning-strong')}>{approvals}</strong>
        {t('workflows:workflowsPage.waitingApproval')}
      </span>
      <span className={stat}>
        <strong className={cn(value, failed > 0 && 'text-danger')}>{failed}</strong>
        {t('workflows:workflowsPage.failed')}
      </span>
      <span className={stat}>
        <strong className={cn(value, 'font-mono')}>{durationLabel(average)}</strong>
        {t('workflows:workflowsPage.averageDuration')}
      </span>
    </div>
  )
}
