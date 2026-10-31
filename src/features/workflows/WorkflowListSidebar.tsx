import { workflowErrorMessage } from './useWorkflowCatalog'
// 工作流列表页的三个视图（规格 v2 第 10 节 A6 第 3 批，功能页模板）：工作流、运行记录与模板，
// 都是 FeatureSection + 列表行，不再是细线卡片表格。行内只放最常用的“运行”，其余操作在 ⋯ 菜单里；
// 点行名打开节点编辑器。运行记录可展开看节点明细与审批；模板行点开即用模板新建。
import { useState } from 'react'
import {
  Check,
  CircleX,
  Copy,
  Download,
  MoreHorizontal,
  Play,
  RefreshCw,
  Square,
  Trash2,
  Workflow as WorkflowIcon,
} from 'lucide-react'
import { FeatureList, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { InlineEmptyState } from '@/components/app/PageState'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Progress } from '@/components/ui/progress'
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
import { RunStatusSquare, RunStatusText, RunStepMeter } from './WorkflowRunStatus'

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

const ACTIVE_RUN = ['running', 'waiting_approval']

export function WorkflowViewTabs({
  value,
  onChange,
  t,
}: {
  value: WorkflowView
  onChange: (value: WorkflowView) => void
  t: WorkflowTranslate
}) {
  // 分段控件：凹陷轨道 + 选中键帽；手机上铺满整行（模板的筛选插槽把每段抬到 44）。
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
  query,
  language,
  t,
  onRun,
  onEdit,
  onDuplicate,
  onExport,
  onDelete,
}: {
  workflows: Workflow[]
  runs: WorkflowRun[]
  busyId: string
  query: string
  language: string
  t: WorkflowTranslate
  onRun: (workflow: Workflow) => void
  onEdit: (workflowId: string) => void
  onDuplicate: (workflow: Workflow) => void
  onExport: (workflow: Workflow) => void
  onDelete: (workflow: Workflow) => void
}) {
  const latestRun = (workflowId: string) =>
    runs
      .filter((run) => run.workflowId === workflowId)
      .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))[0]
  const published = workflows.filter((workflow) => workflow.status === 'published').length

  return (
    <FeatureSection
      title={t('workflows:workflowsPage.workflows')}
      count={workflows.length}
      meta={
        workflows.length
          ? t('workflows:workflowsPage.publishedCount', { count: published })
          : undefined
      }
    >
      {workflows.length ? (
        <FeatureList>
          {workflows.map((workflow) => {
            const run = latestRun(workflow.id)
            const running = ACTIVE_RUN.includes(run?.status || '')
            const runBusy = running || busyId === workflow.id
            return (
              <FeatureRow
                key={workflow.id}
                data-workflow-id={workflow.id}
                icon={<WorkflowIcon />}
                name={workflow.name}
                nameMeta={`v${workflow.revision}`}
                description={
                  workflow.description || <span className="font-mono">{workflow.cwd}</span>
                }
                meta={
                  <>
                    <span>
                      {workflow.status === 'published'
                        ? t('workflows:workflowsPage.published')
                        : t('workflows:workflowsPage.draft')}
                    </span>
                    <span aria-hidden="true">·</span>
                    <span className="truncate">
                      {workflow.visibility === 'shared'
                        ? t('workflows:workflowsPage.shared')
                        : t('workflows:workflowsPage.private')}
                    </span>
                  </>
                }
                status={
                  run ? (
                    <span className="flex min-w-0 items-center gap-2">
                      <RunStatusText status={run.status} label={statusLabel(run.status, t)} />
                      <span className="whitespace-nowrap">
                        {relativeTime(run.startedAt, language)}
                      </span>
                    </span>
                  ) : (
                    t('workflows:workflowsPage.neverRun')
                  )
                }
                // 点行名打开节点编辑器；读屏名沿用原“编辑: 名称”。
                onOpen={() => onEdit(workflow.id)}
                openLabel={`${t('workflows:workflowsPage.edit')}: ${workflow.name}`}
                actions={
                  <>
                    <Button
                      size="icon-sm"
                      variant="outline"
                      disabled={runBusy}
                      title={t('workflows:workflowsPage.run')}
                      aria-label={`${t('workflows:workflowsPage.run')}: ${workflow.name}`}
                      className="max-[650px]:size-11 pointer-coarse:size-11"
                      onClick={() => onRun(workflow)}
                    >
                      {busyId === workflow.id ? <RefreshCw className="animate-spin" /> : <Play />}
                    </Button>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button
                          size="icon-sm"
                          variant="ghost"
                          className="text-content-muted max-[650px]:size-11 pointer-coarse:size-11"
                          aria-label={t('workflows:workflowsPage.moreActionsFor', {
                            name: workflow.name,
                          })}
                        >
                          <MoreHorizontal />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end" className="min-w-44">
                        <DropdownMenuItem
                          className="pointer-coarse:min-h-11 max-[650px]:min-h-11"
                          onSelect={() => onDuplicate(workflow)}
                        >
                          <Copy />
                          {t('workflows:workflowsPage.duplicateWorkflow')}
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          className="pointer-coarse:min-h-11 max-[650px]:min-h-11"
                          onSelect={() => onExport(workflow)}
                        >
                          <Download />
                          {t('workflows:workflowsPage.export')}
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem
                          className="pointer-coarse:min-h-11 max-[650px]:min-h-11"
                          variant="destructive"
                          disabled={running}
                          onSelect={() => onDelete(workflow)}
                        >
                          <Trash2 />
                          {t('workflows:workflowsPage.delete')}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </>
                }
              />
            )
          })}
        </FeatureList>
      ) : (
        <InlineEmptyState
          title={
            query
              ? t('workflows:workflowsPage.noMatchingWorkflows')
              : t('workflows:workflowsPage.noWorkflowsYet')
          }
          description={query ? undefined : t('workflows:workflowsPage.emptyHint')}
        />
      )}
    </FeatureSection>
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
  const ordered = [...runs].sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))
  // 默认展开最近一次运行（与原日志一致）；同一时刻只展开一行。
  const [expandedId, setExpandedId] = useState(ordered[0]?.id || '')
  const waiting = ordered.filter((run) => run.status === 'waiting_approval').length
  return (
    <FeatureSection
      title={t('workflows:workflowsPage.runHistory')}
      count={ordered.length}
      meta={
        waiting ? (
          <span className="font-medium text-st-attn">
            {t('workflows:workflowsPage.waitingApprovalCount', { count: waiting })}
          </span>
        ) : undefined
      }
    >
      {ordered.length ? (
        <FeatureList>
          {ordered.map((run) => {
            const expanded = expandedId === run.id
            const progress = Math.round(
              ((Number(run.completedNodes) || 0) / Math.max(1, Number(run.totalNodes) || 1)) * 100,
            )
            const name = run.workflowName || run.workflowId
            return (
              <FeatureRow
                key={run.id}
                data-workflow-run={run.id}
                icon={<RunStatusSquare status={run.status} className="bg-transparent" />}
                name={name}
                nameMeta={`v${run.workflowRevision || 1}`}
                description={
                  <span className="flex min-w-0 items-center gap-2">
                    <RunStatusText status={run.status} label={statusLabel(run.status, t)} />
                    <span className="truncate tabular-nums">
                      {relativeTime(run.startedAt, language)}
                    </span>
                  </span>
                }
                status={
                  <span className="flex min-w-0 items-center gap-2">
                    <span className="flex w-24 min-w-0 items-center max-[650px]:hidden">
                      {run.nodes?.length ? (
                        <RunStepMeter nodes={run.nodes} className="flex-1" />
                      ) : (
                        <Progress
                          value={run.status === 'completed' ? 100 : progress}
                          className="h-1 flex-1"
                        />
                      )}
                    </span>
                    <span className="font-mono">
                      {Number(run.completedNodes) || 0}/{Number(run.totalNodes) || 0}
                    </span>
                    <span className="font-mono">{durationLabel(run.durationMs)}</span>
                  </span>
                }
                expanded={expanded}
                onExpandedChange={(open) => setExpandedId(open ? run.id : '')}
                toggleLabel={t('workflows:workflowsPage.toggleRunDetails', { name })}
                detail={
                  <div className="workflow-run-detail flex flex-col gap-2.5">
                    {(run.summary || run.error) && (
                      <p className="workflow-run-result flex items-start gap-1.5 text-xs leading-relaxed whitespace-pre-wrap text-content-secondary">
                        {run.error && (
                          <StatusGlyph status="failed" className="mt-px size-3.5 text-content" />
                        )}
                        {run.error ? workflowErrorMessage(run.error) : run.summary}
                      </p>
                    )}
                    {Boolean(run.nodes?.length) && (
                      <ul className="divide-y divide-stroke-soft overflow-hidden rounded-[var(--r-md)] border border-stroke">
                        {(run.nodes || []).map((node) => (
                          <li
                            className={`workflow-run-node grid min-h-[34px] grid-cols-[18px_minmax(0,1fr)_auto] items-center gap-2.5 px-3 py-1.5 max-[650px]:min-h-11 max-[650px]:grid-cols-[18px_minmax(0,1fr)] ${node.status}`}
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
                                <p className="text-xs leading-relaxed whitespace-pre-wrap text-content-muted">
                                  {node.error ? workflowErrorMessage(node.error) : node.summary}
                                </p>
                              )}
                            </span>
                            {node.status === 'waiting_approval' ? (
                              <div className="workflow-approval-actions flex gap-1.5 max-[650px]:col-span-2 max-[650px]:justify-end">
                                <Button
                                  size="xs"
                                  variant="outline"
                                  className="max-[650px]:h-11"
                                  disabled={busyId === node.id}
                                  onClick={() => onApproval(run, node.id, false)}
                                >
                                  <CircleX data-icon="inline-start" />
                                  {t('workflows:workflowsPage.reject')}
                                </Button>
                                <Button
                                  size="xs"
                                  className="max-[650px]:h-11"
                                  disabled={busyId === node.id}
                                  onClick={() => onApproval(run, node.id, true)}
                                >
                                  <Check data-icon="inline-start" />
                                  {t('workflows:workflowsPage.approve')}
                                </Button>
                              </div>
                            ) : (
                              <span className="font-mono text-xs text-content-muted tabular-nums max-[650px]:hidden">
                                {node.status === 'running' ? '…' : durationLabel(node.durationMs)}
                              </span>
                            )}
                          </li>
                        ))}
                      </ul>
                    )}
                    <div className="workflow-run-detail-actions flex gap-1.5 empty:hidden max-[650px]:[&>button]:h-11">
                      {ACTIVE_RUN.includes(run.status) ? (
                        <Button size="sm" variant="outline" onClick={() => onStop(run)}>
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
                }
              />
            )
          })}
        </FeatureList>
      ) : (
        <InlineEmptyState title={t('workflows:workflowsPage.noRunHistory')} />
      )}
    </FeatureSection>
  )
}

export function WorkflowTemplateGallery({
  query,
  t,
  onOpenTemplate,
}: {
  query: string
  t: WorkflowTranslate
  onOpenTemplate: (templateId: string) => void
}) {
  const needle = query.trim().toLocaleLowerCase()
  const templates = WORKFLOW_TEMPLATES.filter((template) =>
    `${templateName(template.id, t)} ${templateDescription(template.id, t)}`
      .toLocaleLowerCase()
      .includes(needle),
  )
  return (
    <FeatureSection
      title={t('workflows:workflowsPage.templates')}
      count={templates.length}
      description={t('workflows:workflowsPage.templatesHint')}
    >
      {templates.length ? (
        <FeatureList className="workflow-template-gallery">
          {templates.map((template) => {
            const Icon = template.Icon
            const name = templateName(template.id, t)
            return (
              <FeatureRow
                key={template.id}
                data-workflow-template={template.id}
                icon={<Icon />}
                name={name}
                description={templateDescription(template.id, t)}
                onOpen={() => onOpenTemplate(template.id)}
                openLabel={t('workflows:workflowsPage.useTemplate', { name })}
              />
            )
          })}
        </FeatureList>
      ) : (
        <InlineEmptyState title={t('workflows:workflowsPage.noMatchingTemplates')} />
      )}
    </FeatureSection>
  )
}

// 运营统计：数字等宽、标签 12 弱化；只有“等待审批”在大于 0 时是琥珀（需要你），失败数保持中性。
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
  return (
    <div className="workflow-operations-strip flex min-w-0 flex-wrap items-center gap-x-5 gap-y-1 text-xs text-content-muted max-[650px]:w-full max-[650px]:justify-between max-[650px]:gap-x-3">
      <span className={stat}>
        {data.limits.running > 0 && (
          <StatusGlyph status="run" className="size-3 self-center text-st-run" />
        )}
        <strong className={value}>{data.limits.running}</strong>
        {t('workflows:workflowsPage.running')}
      </span>
      <span className={cn(stat, approvals > 0 && 'text-st-attn')}>
        <strong className={cn(value, approvals > 0 && 'text-st-attn')}>{approvals}</strong>
        {t('workflows:workflowsPage.waitingApproval')}
      </span>
      <span className={stat}>
        <strong className={value}>{failed}</strong>
        {t('workflows:workflowsPage.failed')}
      </span>
      <span className={stat}>
        <strong className={cn(value, 'font-mono')}>{durationLabel(average)}</strong>
        {t('workflows:workflowsPage.averageDuration')}
      </span>
    </div>
  )
}
