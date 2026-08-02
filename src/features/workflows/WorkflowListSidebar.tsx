import { ChevronRight } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardTitle } from '@/components/ui/card'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { relativeTime } from '@/lib/format'
import type { Workflow, WorkflowRun, WorkflowsData } from './types'
import { WorkflowPreview } from './WorkflowPreview'
import { WorkflowRunActions } from './WorkflowRunControls'
import {
  nodeTypeLabel,
  templateDescription,
  templateName,
  WORKFLOW_FILTERS,
  WORKFLOW_TEMPLATES,
  type WorkflowFilter,
  type WorkflowTranslate,
} from './workflow-templates'

export function WorkflowFilters({
  filter,
  t,
  onChange,
}: {
  filter: WorkflowFilter
  t: WorkflowTranslate
  onChange: (filter: WorkflowFilter) => void
}) {
  return (
    <Tabs
      value={filter}
      onValueChange={(value) => {
        if (WORKFLOW_FILTERS.includes(value as WorkflowFilter)) onChange(value as WorkflowFilter)
      }}
    >
      <TabsList className="workflow-filter-list">
        {WORKFLOW_FILTERS.map((item) => (
          <TabsTrigger value={item} key={item}>
            {item === 'presets'
              ? t('workflows:workflowsPage.presets')
              : item === 'custom'
                ? t('workflows:workflowsPage.custom')
                : item === 'running'
                  ? t('workflows:workflowsPage.running')
                  : item === 'failed'
                    ? t('workflows:workflowsPage.failed')
                    : item === 'draft'
                      ? t('workflows:workflowsPage.draft')
                      : t('workflows:workflowsPage.all')}
          </TabsTrigger>
        ))}
      </TabsList>
    </Tabs>
  )
}

export function WorkflowTemplateSidebar({
  t,
  onOpenTemplate,
}: {
  t: WorkflowTranslate
  onOpenTemplate: (templateId: string) => void
}) {
  return (
    <Card size="sm" className="workflow-card gap-0 py-0">
      <CardContent className="p-3.5">
        <div className="card-head">
          <CardTitle className="workflow-section-title">
            {t('workflows:workflowsPage.commonTemplates')}
          </CardTitle>
          <span>
            {t('workflows:workflowsPage.countTemplates', { count: WORKFLOW_TEMPLATES.length })}
          </span>
        </div>
        <div className="template-grid">
          {WORKFLOW_TEMPLATES.map((template) => {
            const Icon = template.Icon
            return (
              <button onClick={() => onOpenTemplate(template.id)} key={template.id}>
                <span className="list-icon">
                  <Icon size={15} />
                </span>
                <span>
                  <strong>{templateName(template.id, t)}</strong>
                  <small>{templateDescription(template.id, t)}</small>
                </span>
                <ChevronRight size={14} />
              </button>
            )
          })}
        </div>
      </CardContent>
    </Card>
  )
}

export function WorkflowPreviewPanel({
  workflow,
  t,
  onStartBlank,
}: {
  workflow?: Workflow
  t: WorkflowTranslate
  onStartBlank: () => void
}) {
  return (
    <Card size="sm" className="workflow-card workflow-preview gap-0 py-0">
      <CardContent className="p-3.5">
        <div className="card-head">
          <div>
            <CardTitle className="workflow-section-title">
              {t('workflows:workflowsPage.customWorkflow')}
            </CardTitle>
            {workflow && (
              <small>
                {workflow.name} ·{' '}
                {workflow.status === 'published'
                  ? t('workflows:workflowsPage.published')
                  : t('workflows:workflowsPage.draft')}
              </small>
            )}
          </div>
          <Button size="xs" variant="link" onClick={onStartBlank}>
            {t('workflows:workflowsPage.startBlank')}
          </Button>
        </div>
        {workflow?.nodes.length ? (
          <WorkflowPreview
            nodes={workflow.nodes}
            edges={workflow.edges}
            nodeTypeLabel={(kind) => nodeTypeLabel(kind, t)}
          />
        ) : (
          <div className="channel-route-empty compact">
            <strong>{t('workflows:workflowsPage.noCustomWorkflowYet')}</strong>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function workflowStatusCopy(
  workflow: Workflow,
  run: WorkflowRun | undefined,
  language: string,
  t: WorkflowTranslate,
) {
  if (run?.status === 'running') {
    return t('workflows:workflowsPage.runningNode', {
      node: run.currentNodeLabel || t('workflows:workflowsPage.preparing'),
    })
  }
  if (workflow.lastRunAt) {
    const status =
      workflow.lastStatus === 'completed'
        ? t('workflows:workflowsPage.completed')
        : workflow.lastStatus === 'failed'
          ? t('workflows:workflowsPage.failed')
          : workflow.lastStatus === 'cancelled'
            ? t('workflows:workflowsPage.stopped')
            : t('workflows:workflowsPage.draft')
    return `${status} · ${relativeTime(workflow.lastRunAt, language)}`
  }
  return workflow.status === 'published'
    ? t('workflows:workflowsPage.published')
    : t('workflows:workflowsPage.draft')
}

export function WorkflowListPanel({
  workflows,
  filter,
  busyId,
  language,
  latestRun,
  t,
  onRun,
  onStop,
  onEdit,
  onDelete,
}: {
  workflows: Workflow[]
  filter: WorkflowFilter
  busyId: string
  language: string
  latestRun: (workflowId: string) => WorkflowRun | undefined
  t: WorkflowTranslate
  onRun: (workflow: Workflow) => void
  onStop: (run: WorkflowRun) => void
  onEdit: (workflowId: string) => void
  onDelete: (workflow: Workflow) => void
}) {
  return (
    <Card size="sm" className="workflow-card gap-0 py-0">
      <CardContent className="p-3.5">
        <div className="card-head">
          <CardTitle className="workflow-section-title">
            {t('workflows:workflowsPage.workflows')}
          </CardTitle>
          <span>{t('workflows:workflowsPage.countWorkflows', { count: workflows.length })}</span>
        </div>
        {workflows.length ? (
          workflows.map((workflow) => {
            const run = latestRun(workflow.id)
            return (
              <div className="run-row" key={workflow.id}>
                <span>
                  <strong>{workflow.name}</strong>
                  <small>{workflowStatusCopy(workflow, run, language, t)}</small>
                </span>
                <WorkflowRunActions
                  workflow={workflow}
                  run={run}
                  busyId={busyId}
                  t={t}
                  onRun={onRun}
                  onStop={onStop}
                  onEdit={onEdit}
                  onDelete={onDelete}
                />
              </div>
            )
          })
        ) : (
          <div className="channel-route-empty compact">
            <strong>
              {filter === 'presets'
                ? t('workflows:workflowsPage.chooseATemplateAboveToGetStarted')
                : t('workflows:workflowsPage.noMatchingWorkflows')}
            </strong>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

export function WorkflowQueueSummary({ data, t }: { data: WorkflowsData; t: WorkflowTranslate }) {
  const published = data.workflows.filter((workflow) => workflow.status === 'published').length
  const notificationCount = Object.values(data.notificationTargets).filter(
    (target) => target.enabled,
  ).length
  const rows = [
    [
      t('workflows:workflowsPage.maximumConcurrency'),
      String(data.limits.maxConcurrent || 4),
      t('workflows:workflowsPage.countCurrentlyRunning', { count: data.limits.running || 0 }),
    ],
    [
      t('workflows:workflowsPage.published'),
      String(published),
      t('workflows:workflowsPage.countWorkflowsTotal', { count: data.workflows.length }),
    ],
    [
      t('workflows:workflowsPage.retryOnFailure'),
      t('workflows:workflowsPage.upTo3TimesPerNode'),
      t('workflows:workflowsPage.configurablePerNode'),
    ],
    [
      t('workflows:workflowsPage.completionDelivery'),
      notificationCount
        ? t('workflows:workflowsPage.enabled')
        : t('workflows:workflowsPage.notEnabled'),
      t('workflows:workflowsPage.countAvailableChannels', { count: notificationCount }),
    ],
  ]
  return (
    <Card size="sm" className="workflow-card gap-0 py-0">
      <CardContent className="p-3.5">
        <CardTitle className="workflow-section-title">
          {t('workflows:workflowsPage.queueAndLimits')}
        </CardTitle>
        {rows.map((row) => (
          <div className="setting-row" key={row[0]}>
            <span>
              <strong>{row[0]}</strong>
              <small>{row[2]}</small>
            </span>
            <strong className="workflow-setting-value">{row[1]}</strong>
          </div>
        ))}
      </CardContent>
    </Card>
  )
}
