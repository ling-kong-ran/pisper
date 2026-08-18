// 工作流运行控制：启动/停止运行、查看运行状态与日志。
import { AlertTriangle, CheckCircle2, Pencil, Play, RefreshCw, Square, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardTitle } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'
import { relativeTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { Workflow, WorkflowRun } from './types'
import type { WorkflowTranslate } from './workflow-templates'

import { AppNotice } from '@/components/ui/app-primitives'

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
          'workflow-run-progress max-[650px]:[.workflows-page_&]:[grid-column:1] h-[5px] bg-[var(--progress-track)]',
          run?.status === 'completed' &&
            '[&_[data-slot=progress-indicator]]:bg-[var(--status-green)]',
          run?.status === 'failed' && '[&_[data-slot=progress-indicator]]:bg-[var(--amber)]',
        )}
      />
      <em>{progress}%</em>
      <div className="workflow-run-actions max-[650px]:[.workflows-page_&]:[grid-column:1/-1] max-[650px]:[.workflows-page_&]:flex-wrap flex items-center gap-[3px]">
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
    <AppNotice role="status">
      <RefreshCw className="animate-spin" size={16} />
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
    </AppNotice>
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
    <Card
      size="sm"
      className="workflow-card [&_h2]:text-[16px] [&_h2]:tracking-[-.02em] [.detail-stack_>_&]:[flex:0_0_auto] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--panel)] text-[var(--text)] shadow-[0_1px_2px_var(--sh-edge),0_14px_32px_-24px_var(--shadow)] gap-0 py-0"
    >
      <CardContent className="p-3.5">
        <CardTitle className="workflow-section-title [.selection-list_&]:mb-[8px] [.node-library_&]:mb-[8px] text-[var(--text-soft)] text-[13px] font-[700] leading-[1.4]">
          {t('workflows:workflowsPage.latestRun')}
        </CardTitle>
        {run ? (
          <div
            className={`activity-row [&_span]:flex [&_span]:min-w-0 [&_span]:flex-col [&_span]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_span]:text-[var(--text)] [&.failed]:text-[var(--danger)] [&.running]:text-[var(--star-strong)] grid grid-cols-[auto_minmax(0,1fr)] items-center gap-[9px] [border-top:1px_solid_var(--stroke-soft)] [padding:9px_2px] text-[var(--success)] ${run.status}`}
          >
            {run.status === 'running' ? (
              <RefreshCw className="animate-spin" size={15} />
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
          <p className="muted-copy m-[8px_0_14px] text-[var(--text-muted)] text-[12px] leading-[1.55]">
            {t('workflows:workflowsPage.noRunHistory')}
          </p>
        )}
      </CardContent>
    </Card>
  )
}
