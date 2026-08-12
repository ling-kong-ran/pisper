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
      <TabsList className="workflow-filter-list">
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
    <Card size="sm" className="workflow-card workflow-assets-panel gap-0 py-0">
      <CardContent className="p-0">
        <div className="workflow-table-head">
          <div>
            <CardTitle className="workflow-section-title">
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
          <div className="workflow-asset-table">
            {workflows.map((workflow) => {
              const run = latestRun(workflow.id)
              const running = ['running', 'waiting_approval'].includes(run?.status || '')
              return (
                <div className="workflow-asset-row" key={workflow.id}>
                  <button className="workflow-asset-main" onClick={() => onEdit(workflow.id)}>
                    <strong>{workflow.name}</strong>
                    <small>{workflow.description || workflow.cwd}</small>
                  </button>
                  <span className={`workflow-state ${workflow.status}`}>
                    {workflow.status === 'published'
                      ? t('workflows:workflowsPage.published')
                      : t('workflows:workflowsPage.draft')}
                    <small>v{workflow.revision}</small>
                  </span>
                  <span className={`workflow-state ${run?.status || 'idle'}`}>
                    {run ? statusLabel(run.status, t) : t('workflows:workflowsPage.neverRun')}
                    <small>
                      {run ? relativeTime(run.startedAt, language) : workflow.visibility}
                    </small>
                  </span>
                  <div className="workflow-row-actions">
                    <Button
                      size="icon-xs"
                      variant={running ? 'secondary' : 'outline'}
                      disabled={running || busyId === workflow.id}
                      title={t('workflows:workflowsPage.run')}
                      onClick={() => onRun(workflow)}
                    >
                      {busyId === workflow.id ? (
                        <RefreshCw className="spin" />
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
          <div className="channel-route-empty compact">
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
    <div className="workflow-run-history">
      {ordered.map((run) => {
        const expanded = expandedId === run.id
        const progress = Math.round(
          ((Number(run.completedNodes) || 0) / Math.max(1, Number(run.totalNodes) || 1)) * 100,
        )
        return (
          <Card size="sm" className="workflow-card workflow-run-record gap-0 py-0" key={run.id}>
            <CardContent className="p-0">
              <button
                className="workflow-run-summary"
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
                <div className="workflow-run-detail">
                  {(run.summary || run.error) && (
                    <div className={`workflow-run-result ${run.error ? 'failed' : ''}`}>
                      {run.error || run.summary}
                    </div>
                  )}
                  <div className="workflow-run-node-list">
                    {(run.nodes || []).map((node) => (
                      <div className={`workflow-run-node ${node.status}`} key={node.id}>
                        <span className="workflow-node-status">
                          {node.status === 'completed' ? (
                            <Check size={14} />
                          ) : node.status === 'failed' ? (
                            <CircleX size={14} />
                          ) : node.status === 'running' || node.status === 'waiting_approval' ? (
                            <RefreshCw
                              className={node.status === 'running' ? 'spin' : ''}
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
                          <div className="workflow-approval-actions">
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
                  <div className="workflow-run-detail-actions">
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
        <Card size="sm" className="workflow-card empty-state">
          <strong>{t('workflows:workflowsPage.noRunHistory')}</strong>
        </Card>
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
    <div className="workflow-template-gallery">
      {WORKFLOW_TEMPLATES.map((template) => {
        const Icon = template.Icon
        return (
          <button key={template.id} onClick={() => onOpenTemplate(template.id)}>
            <span className="list-icon">
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
    <div className="workflow-operations-strip">
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
