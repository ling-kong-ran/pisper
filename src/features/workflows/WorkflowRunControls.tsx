import { AlertTriangle, CheckCircle2, Pencil, Play, RefreshCw, Square, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardTitle } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'
import { relativeTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { Workflow, WorkflowRun } from './types'
import type { WorkflowTranslate } from './workflow-templates'

function workflowRunProgress(run?: WorkflowRun) {
  if (!run) return 0
  if (run.status === 'completed') return 100
  return Math.round(
    ((Number(run.completedNodes) || 0) / Math.max(1, Number(run.totalNodes) || 1)) * 100,
  )
}

function durationLabel(durationMs?: number) {
  const seconds = Math.max(0, Math.round((Number(durationMs) || 0) / 1000))
  return seconds < 60 ? `${seconds} 秒` : `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`
}

export function WorkflowRunActions({
  workflow,
  run,
  busyId,
  t,
  onRun,
  onStop,
  onEdit,
  onDelete,
}: {
  workflow: Workflow
  run?: WorkflowRun
  busyId: string
  t: WorkflowTranslate
  onRun: (workflow: Workflow) => void
  onStop: (run: WorkflowRun) => void
  onEdit: (workflowId: string) => void
  onDelete: (workflow: Workflow) => void
}) {
  const running = run?.status === 'running'
  const progress = workflowRunProgress(run)
  return (
    <>
      <Progress
        value={progress}
        aria-label={`${workflow.name}: ${progress}%`}
        className={cn(
          'workflow-run-progress',
          run?.status === 'completed' &&
            '[&_[data-slot=progress-indicator]]:bg-[var(--status-green)]',
          run?.status === 'failed' && '[&_[data-slot=progress-indicator]]:bg-[var(--amber)]',
        )}
      />
      <em>{progress}%</em>
      <div className="workflow-run-actions">
        {running ? (
          <Button
            size="xs"
            variant="destructive"
            disabled={busyId === run.id}
            onClick={() => onStop(run)}
          >
            <Square data-icon="inline-start" />
            {t('workflows:workflowsPage.stop')}
          </Button>
        ) : (
          <Button
            size="xs"
            variant="outline"
            disabled={busyId === workflow.id}
            onClick={() => onRun(workflow)}
          >
            <Play data-icon="inline-start" />
            {t('workflows:workflowsPage.run')}
          </Button>
        )}
        <Button size="xs" variant="ghost" onClick={() => onEdit(workflow.id)}>
          <Pencil data-icon="inline-start" />
          {t('workflows:workflowsPage.edit')}
        </Button>
        <Button
          size="xs"
          variant="destructive"
          disabled={running || busyId === workflow.id}
          onClick={() => onDelete(workflow)}
        >
          <Trash2 data-icon="inline-start" />
          {t('workflows:workflowsPage.delete')}
        </Button>
      </div>
    </>
  )
}

export function WorkflowRunningNotice({ run, t }: { run: WorkflowRun; t: WorkflowTranslate }) {
  return (
    <div className="permission-note" role="status">
      <RefreshCw className="spin" size={16} />
      <span>
        <strong>{t('workflows:workflowsPage.workflowRunning')}</strong>
        <small>
          {t('workflows:workflowsPage.runningNodeCompletedTotalCompleted', {
            node: run.currentNodeLabel || t('workflows:workflowsPage.preparing'),
            completed: run.completedNodes,
            total: run.totalNodes,
          })}
        </small>
      </span>
    </div>
  )
}

export function WorkflowLatestRun({
  run,
  language,
  t,
}: {
  run?: WorkflowRun
  language: string
  t: WorkflowTranslate
}) {
  return (
    <Card size="sm" className="workflow-card gap-0 py-0">
      <CardContent className="p-3.5">
        <CardTitle className="workflow-section-title">
          {t('workflows:workflowsPage.latestRun')}
        </CardTitle>
        {run ? (
          <div className={`activity-row ${run.status}`}>
            {run.status === 'running' ? (
              <RefreshCw className="spin" size={15} />
            ) : run.status === 'completed' ? (
              <CheckCircle2 size={15} />
            ) : (
              <AlertTriangle size={15} />
            )}
            <span>
              <strong>
                {run.status === 'completed'
                  ? run.summary || t('workflows:workflowsPage.workflowCompleted')
                  : run.status === 'running'
                    ? run.currentNodeLabel || t('workflows:workflowsPage.running')
                    : run.error || t('workflows:workflowsPage.workflowFailed')}
              </strong>
              <small>
                {relativeTime(run.startedAt, language)} · {durationLabel(run.durationMs)}
              </small>
            </span>
          </div>
        ) : (
          <p className="muted-copy">{t('workflows:workflowsPage.noRunHistory')}</p>
        )}
      </CardContent>
    </Card>
  )
}
