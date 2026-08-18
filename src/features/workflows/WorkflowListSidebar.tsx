// 工作流列表侧栏：按分组展示已保存的工作流，支持新建/重命名/删除。
import { useRef, useState } from 'react'
import {
  Check,
  ChevronDown,
  ChevronRight,
  CircleX,
  Copy,
  Download,
  FileUp,
  Pencil,
  Play,
  RefreshCw,
  Square,
  Trash2,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardTitle } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { relativeTime } from '@/lib/format'
import type { Workflow, WorkflowRun, WorkflowsData } from './types'
import {
  templateDescription,
  templateName,
  WORKFLOW_TEMPLATES,
  type WorkflowTranslate,
} from './workflow-templates'

import { AppEmptyState } from '@/components/ui/app-primitives'

export type WorkflowView = 'workflows' | 'runs' | 'templates'

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

export function WorkflowViewTabs({
  value,
  onChange,
  t,
}: {
  value: WorkflowView
  onChange: (value: WorkflowView) => void
  t: WorkflowTranslate
}) {
  return (
    <Tabs value={value} onValueChange={(next) => onChange(next as WorkflowView)}>
      <TabsList className="workflow-filter-list [&_[data-slot='tabs-trigger']]:min-w-[56px] [&_[data-slot='tabs-trigger']]:flex-none [&_[data-slot='tabs-trigger']]:[padding-inline:12px] [&_[data-slot='tabs-trigger']]:text-[12px] w-fit max-w-[100%] overflow-x-auto [scrollbar-width:none]">
        <TabsTrigger value="workflows">{t('workflows:workflowsPage.workflows')}</TabsTrigger>
        <TabsTrigger value="runs">{t('workflows:workflowsPage.runHistory')}</TabsTrigger>
        <TabsTrigger value="templates">{t('workflows:workflowsPage.templates')}</TabsTrigger>
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
  onImport: (value: unknown) => void
  onDelete: (workflow: Workflow) => void
}) {
  const fileRef = useRef<HTMLInputElement>(null)
  const latestRun = (workflowId: string) =>
    runs
      .filter((run) => run.workflowId === workflowId)
      .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt))[0]

  return (
    <Card
      size="sm"
      className="workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] workflow-assets-panel overflow-hidden gap-0 py-0"
    >
      <CardContent className="p-0">
        <div className="workflow-table-head [&_>_div]:flex [&_>_div]:min-w-0 [&_>_div]:flex-col [&_>_div]:gap-[2px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] flex min-h-[58px] items-center justify-between gap-[12px] [border-bottom:1px_solid_var(--stroke-soft)] [padding:10px_14px]">
          <div>
            <CardTitle className="workflow-section-title [.selection-list_&]:mb-[8px] [.node-library_&]:mb-[8px] text-[var(--text-soft)] text-[13px] font-[700] leading-[1.4]">
              {t('workflows:workflowsPage.workflows')}
            </CardTitle>
            <small>
              {t('workflows:workflowsPage.countWorkflows', { count: workflows.length })}
            </small>
          </div>
          <Button size="sm" variant="outline" onClick={() => fileRef.current?.click()}>
            <FileUp data-icon="inline-start" />
            {t('workflows:workflowsPage.import')}
          </Button>
          <input
            ref={fileRef}
            className="sr-only"
            type="file"
            accept="application/json,.json"
            onChange={async (event) => {
              const file = event.target.files?.[0]
              if (!file) return
              try {
                onImport(JSON.parse(await file.text()))
              } catch {
                onImport(null)
              } finally {
                event.currentTarget.value = ''
              }
            }}
          />
        </div>
        {workflows.length ? (
          <div className="flex flex-col">
            {workflows.map((workflow) => {
              const run = latestRun(workflow.id)
              const running = ['running', 'waiting_approval'].includes(run?.status || '')
              return (
                <div
                  className="workflow-asset-row first:[border-top:0] max-[650px]:grid-cols-[minmax(0,1fr)_92px] max-[650px]:gap-[8px_12px] max-[650px]:[padding-block:10px] grid min-h-[62px] grid-cols-[minmax(220px,1fr)_110px_150px_auto] items-center gap-[14px] [border-top:1px_solid_var(--stroke-soft)] [padding:8px_12px_8px_14px]"
                  key={workflow.id}
                >
                  <button
                    className="workflow-asset-main [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] flex min-w-0 flex-col gap-[3px] border-0 bg-transparent text-[var(--text)] text-left cursor-pointer"
                    onClick={() => onEdit(workflow.id)}
                  >
                    <strong>{workflow.name}</strong>
                    <small>{workflow.description || workflow.cwd}</small>
                  </button>
                  <span
                    className={`workflow-state [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[10px] [&_small]:font-[400] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&.running]:text-[var(--warning-strong)] [&.waiting_approval]:text-[var(--warning-strong)] [&.completed]:text-[var(--success)] [&.published]:text-[var(--success)] [&.failed]:text-[var(--danger)] max-[650px]:[.workflow-asset-row_>_&:nth-of-type(2)]:[grid-column:1] flex min-w-0 flex-col gap-[2px] text-[var(--text-secondary)] text-[11px] font-[600] ${workflow.status}`}
                  >
                    {workflow.status === 'published'
                      ? t('workflows:workflowsPage.published')
                      : t('workflows:workflowsPage.draft')}
                    <small>v{workflow.revision}</small>
                  </span>
                  <span
                    className={`workflow-state [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[10px] [&_small]:font-[400] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&.running]:text-[var(--warning-strong)] [&.waiting_approval]:text-[var(--warning-strong)] [&.completed]:text-[var(--success)] [&.published]:text-[var(--success)] [&.failed]:text-[var(--danger)] max-[650px]:[.workflow-asset-row_>_&:nth-of-type(2)]:[grid-column:1] flex min-w-0 flex-col gap-[2px] text-[var(--text-secondary)] text-[11px] font-[600] ${run?.status || 'idle'}`}
                  >
                    {run ? statusLabel(run.status, t) : t('workflows:workflowsPage.neverRun')}
                    <small>
                      {run ? relativeTime(run.startedAt, language) : workflow.visibility}
                    </small>
                  </span>
                  <div className="workflow-row-actions max-[650px]:[grid-column:2] max-[650px]:[grid-row:1/3] max-[650px]:flex-wrap flex items-center justify-end gap-[3px]">
                    <Button
                      size="icon-xs"
                      variant={running ? 'secondary' : 'outline'}
                      disabled={running || busyId === workflow.id}
                      title={t('workflows:workflowsPage.run')}
                      onClick={() => onRun(workflow)}
                    >
                      {busyId === workflow.id ? (
                        <RefreshCw className="animate-spin" />
                      ) : (
                        <Play fill="currentColor" />
                      )}
                    </Button>
                    <Button
                      size="icon-xs"
                      variant="ghost"
                      title={t('workflows:workflowsPage.edit')}
                      onClick={() => onEdit(workflow.id)}
                    >
                      <Pencil />
                    </Button>
                    <Button
                      size="icon-xs"
                      variant="ghost"
                      title={t('workflows:workflowsPage.duplicateWorkflow')}
                      onClick={() => onDuplicate(workflow)}
                    >
                      <Copy />
                    </Button>
                    <Button
                      size="icon-xs"
                      variant="ghost"
                      title={t('workflows:workflowsPage.export')}
                      onClick={() => onExport(workflow)}
                    >
                      <Download />
                    </Button>
                    <Button
                      size="icon-xs"
                      variant="ghost"
                      disabled={running}
                      title={t('workflows:workflowsPage.delete')}
                      onClick={() => onDelete(workflow)}
                    >
                      <Trash2 />
                    </Button>
                  </div>
                </div>
              )
            })}
          </div>
        ) : (
          <div className="channel-route-empty [&_strong]:mt-[9px] [&_strong]:text-[var(--text)] [&_strong]:text-[12px] [&_span]:mt-[4px] [&_span]:text-[13px] [&.compact]:min-h-[110px] [.workflow-assets-panel_&]:min-h-[150px] [.workflow-assets-panel_&]:border-0 [.workflow-assets-panel_&]:bg-transparent grid min-h-[185px] place-content-center justify-items-center text-[var(--text-muted)] text-center compact">
            <strong>{t('workflows:workflowsPage.noMatchingWorkflows')}</strong>
          </div>
        )}
      </CardContent>
    </Card>
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
  return (
    <div className="flex flex-col gap-[8px]">
      {ordered.map((run) => {
        const expanded = expandedId === run.id
        const progress = Math.round(
          ((Number(run.completedNodes) || 0) / Math.max(1, Number(run.totalNodes) || 1)) * 100,
        )
        return (
          <Card
            size="sm"
            className="workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] overflow-hidden gap-0 py-0"
            key={run.id}
          >
            <CardContent className="p-0">
              <button
                className="workflow-run-summary hover:bg-[var(--surface-hover)] [&_>_span]:flex [&_>_span]:min-w-0 [&_>_span]:flex-col [&_>_span]:gap-[2px] [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_small]:[font-style:normal] [&_em]:text-[var(--text-muted)] [&_em]:text-[11px] [&_em]:[font-style:normal] [&_[data-slot='progress']]:h-[5px] max-[650px]:grid-cols-[18px_minmax(0,1fr)_54px] max-[650px]:gap-[8px] max-[650px]:[&_[data-slot='progress']]:[grid-column:2/4] grid w-full min-h-[58px] grid-cols-[18px_minmax(180px,1fr)_minmax(120px,240px)_70px] items-center gap-[12px] border-0 bg-transparent [padding:9px_14px] text-[var(--text)] text-left cursor-pointer"
                onClick={() => setExpandedId(expanded ? '' : run.id)}
              >
                {expanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
                <span>
                  <strong>{run.workflowName || run.workflowId}</strong>
                  <small>
                    {statusLabel(run.status, t)} · {relativeTime(run.startedAt, language)} · v
                    {run.workflowRevision || 1}
                  </small>
                </span>
                <Progress value={run.status === 'completed' ? 100 : progress} />
                <em>{durationLabel(run.durationMs)}</em>
              </button>
              {expanded && (
                <div className="workflow-run-detail max-[650px]:pl-[14px] flex flex-col gap-[10px] [border-top:1px_solid_var(--stroke-soft)] [padding:12px_14px_14px_44px]">
                  {(run.summary || run.error) && (
                    <div
                      className={`workflow-run-result [&.failed]:border-[var(--danger)] [&.failed]:text-[var(--danger)] [border-left:2px_solid_var(--success)] [padding:4px_8px] text-[var(--text-secondary)] text-[12px] whitespace-pre-wrap ${run.error ? 'failed' : ''}`}
                    >
                      {run.error || run.summary}
                    </div>
                  )}
                  <div className="flex flex-col">
                    {(run.nodes || []).map((node) => (
                      <div
                        className={`workflow-run-node first:[border-top:0] [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[2px] [&_strong]:text-[12px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_p]:text-[var(--text-muted)] [&_p]:text-[11px] [&_p]:mt-[3px] [&_p]:whitespace-pre-wrap max-[650px]:grid-cols-[18px_minmax(0,1fr)] max-[650px]:[&_>_div]:[grid-column:2] grid min-h-[46px] grid-cols-[20px_minmax(0,1fr)_auto] items-center gap-[8px] [border-top:1px_solid_var(--stroke-soft)] [padding:6px_0] ${node.status}`}
                        key={node.id}
                      >
                        <span className="workflow-node-status [.workflow-run-node.completed_&]:text-[var(--success)] [.workflow-run-node.failed_&]:text-[var(--danger)] grid place-items-center text-[var(--text-muted)]">
                          {node.status === 'completed' ? (
                            <Check size={14} />
                          ) : node.status === 'failed' ? (
                            <CircleX size={14} />
                          ) : node.status === 'running' || node.status === 'waiting_approval' ? (
                            <RefreshCw
                              className={node.status === 'running' ? 'animate-spin' : ''}
                              size={14}
                            />
                          ) : (
                            <Square size={12} />
                          )}
                        </span>
                        <span>
                          <strong>{node.label}</strong>
                          <small>
                            {statusLabel(node.status, t)} · {durationLabel(node.durationMs)} ·{' '}
                            {t('workflows:workflowsPage.countAttempts', {
                              count: node.attempts || 0,
                            })}
                          </small>
                          {(node.error || node.summary) && <p>{node.error || node.summary}</p>}
                        </span>
                        {node.status === 'waiting_approval' && (
                          <div className="workflow-approval-actions flex gap-[6px]">
                            <Button
                              size="xs"
                              disabled={busyId === node.id}
                              onClick={() => onApproval(run, node.id, true)}
                            >
                              <Check data-icon="inline-start" />
                              {t('workflows:workflowsPage.approve')}
                            </Button>
                            <Button
                              size="xs"
                              variant="destructive"
                              disabled={busyId === node.id}
                              onClick={() => onApproval(run, node.id, false)}
                            >
                              <CircleX data-icon="inline-start" />
                              {t('workflows:workflowsPage.reject')}
                            </Button>
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                  <div className="workflow-run-detail-actions flex gap-[6px]">
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
            </CardContent>
          </Card>
        )
      })}
      {!ordered.length && (
        <AppEmptyState
          size="sm"
          className="workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)]"
        >
          <strong>{t('workflows:workflowsPage.noRunHistory')}</strong>
        </AppEmptyState>
      )}
    </div>
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
    <div className="workflow-template-gallery [&_>_button]:grid [&_>_button]:min-h-[72px] [&_>_button]:grid-cols-[34px_minmax(0,1fr)_18px] [&_>_button]:items-center [&_>_button]:gap-[10px] [&_>_button]:[border:1px_solid_var(--stroke)] [&_>_button]:rounded-[var(--r-sm)] [&_>_button]:bg-[var(--panel)] [&_>_button]:p-[11px] [&_>_button]:text-[var(--text)] [&_>_button]:text-left [&_>_button]:cursor-pointer [&_>_button:hover]:border-[var(--accent-border)] [&_>_button:hover]:bg-[var(--surface-hover)] [&_button_>_span:nth-child(2)]:flex [&_button_>_span:nth-child(2)]:min-w-0 [&_button_>_span:nth-child(2)]:flex-col [&_button_>_span:nth-child(2)]:gap-[3px] [&_strong]:text-[13px] [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(2,minmax(0,1fr))] gap-[8px]">
      {WORKFLOW_TEMPLATES.map((template) => {
        const Icon = template.Icon
        return (
          <button key={template.id} onClick={() => onOpenTemplate(template.id)}>
            <span className="list-icon [.chat-resource-list_&]:grid [.chat-resource-list_&]:w-[28px] [.chat-resource-list_&]:h-[28px] [.chat-resource-list_&]:place-items-center [.chat-resource-list_&]:rounded-[var(--r-sm)] [.chat-resource-list_&]:bg-[var(--surface-subtle)] [.chat-resource-list_&]:text-[var(--star-strong)] [.session-workflow-summary_&]:grid [.session-workflow-summary_&]:w-[28px] [.session-workflow-summary_&]:h-[28px] [.session-workflow-summary_&]:place-items-center [.session-workflow-summary_&]:rounded-[var(--r-sm)] [.session-workflow-summary_&]:bg-[var(--surface-subtle)] [.session-workflow-summary_&]:text-[var(--star-strong)] grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)] [.workflow-template-gallery_&]:grid [.workflow-template-gallery_&]:w-[32px] [.workflow-template-gallery_&]:h-[32px] [.workflow-template-gallery_&]:place-items-center [.workflow-template-gallery_&]:rounded-[var(--r-sm)] [.workflow-template-gallery_&]:bg-[var(--surface-subtle)] [.workflow-template-gallery_&]:text-[var(--star-strong)]">
              <Icon size={16} />
            </span>
            <span>
              <strong>{templateName(template.id, t)}</strong>
              <small>{templateDescription(template.id, t)}</small>
            </span>
            <ChevronRight size={15} />
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
  return (
    <div className="workflow-operations-strip [&_>_span]:inline-flex [&_>_span]:[align-items:baseline] [&_>_span]:gap-[5px] [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[11px] [&_>_span]:whitespace-nowrap [&_strong]:text-[var(--text)] [&_strong]:text-[13px] [&_strong]:[font-variant-numeric:tabular-nums] max-[650px]:justify-between max-[650px]:gap-[6px] max-[650px]:overflow-x-auto max-[650px]:[&_>_span]:flex-col max-[650px]:[&_>_span]:gap-[0] flex min-w-0 items-center gap-[18px]">
      <span>
        <strong>{data.limits.running}</strong>
        {t('workflows:workflowsPage.running')}
      </span>
      <span>
        <strong>{approvals}</strong>
        {t('workflows:workflowsPage.waitingApproval')}
      </span>
      <span>
        <strong>{failed}</strong>
        {t('workflows:workflowsPage.failed')}
      </span>
      <span>
        <strong>{durationLabel(average)}</strong>
        {t('workflows:workflowsPage.averageDuration')}
      </span>
    </div>
  )
}
