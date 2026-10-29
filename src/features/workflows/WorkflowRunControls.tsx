import { workflowErrorMessage } from './useWorkflowCatalog'
// 工作流运行控制：启动/停止运行、查看运行状态与日志。
import { Clock3, Pencil, Play, Square, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { OrbitSpinner } from '@/components/ui/spinner'
import { relativeTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { Workflow, WorkflowRun } from './types'
import type { WorkflowTranslate } from './workflow-templates'
import { RunStatusSquare } from './WorkflowRunStatus'

function workflowRunProgress(run?: WorkflowRun) {
  if (!run) return 0
  if (run.status === 'completed') return 100
  return Math.round(
    ((Number(run.completedNodes) || 0) / Math.max(1, Number(run.totalNodes) || 1)) * 100,
  )
}

// 时长用紧凑的拉丁单位（42s / 1m 36s），中英文界面一致且配合等宽数字不跳动。
function durationLabel(durationMs?: number) {
  const seconds = Math.max(0, Math.round((Number(durationMs) || 0) / 1000))
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`
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
  const running = run?.status === 'running' || run?.status === 'waiting_approval'
  const progress = workflowRunProgress(run)
  return (
    <>
      <Progress
        value={progress}
        aria-label={`${workflow.name}: ${progress}%`}
        className={cn(
          'workflow-run-progress h-1',
          run?.status === 'completed' && '[&_[data-slot=progress-indicator]]:bg-success',
          run?.status === 'failed' && '[&_[data-slot=progress-indicator]]:bg-danger',
        )}
      />
      <em className="text-xs text-content-muted not-italic tabular-nums">{progress}%</em>
      <div className="workflow-run-actions flex flex-wrap items-center gap-1">
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

// 编辑器顶部的运行条：强调色软底 + 轨道指示 + 强调色「运行中」，右侧等宽计数，
// 底边一条随已完成节点推进的强调色进度细线；等待审批时换成警示色时钟，不伪装成运行中。
export function WorkflowRunningNotice({ run, t }: { run: WorkflowRun; t: WorkflowTranslate }) {
  const waiting = run.status === 'waiting_approval'
  const progress = workflowRunProgress(run)
  return (
    <div
      role="status"
      className={cn(
        'relative flex min-h-9 shrink-0 items-center gap-2 overflow-hidden border-b border-stroke-soft px-3 text-sm max-[650px]:min-h-11',
        waiting ? 'bg-warning-soft' : 'bg-star-soft',
      )}
    >
      {waiting ? (
        <Clock3 className="size-4 shrink-0 text-warning-strong" aria-hidden="true" />
      ) : (
        <OrbitSpinner />
      )}
      <strong
        className={cn('shrink-0 font-medium', waiting ? 'text-warning-strong' : 'text-accent-fg')}
      >
        {waiting
          ? t('workflows:workflowsPage.waitingApproval')
          : t('workflows:workflowsPage.workflowRunning')}
      </strong>
      <span className="min-w-0 truncate text-content-secondary tabular-nums">
        {t('workflows:workflowsPage.runningNodeCompletedTotalCompleted', {
          node: run.currentNodeLabel || t('workflows:workflowsPage.preparing'),
          completed: run.completedNodes,
          total: run.totalNodes,
        })}
      </span>
      <span className="ml-auto shrink-0 font-mono text-xs text-content-muted tabular-nums">
        {durationLabel(run.durationMs)}
      </span>
      <i
        aria-hidden="true"
        className="absolute inset-x-0 bottom-0 h-0.5 origin-left bg-star transition-transform duration-(--d-base) ease-precise"
        style={{ transform: `scaleX(${progress / 100})` }}
      />
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
  if (!run)
    return <p className="text-xs text-content-muted">{t('workflows:workflowsPage.noRunHistory')}</p>
  return (
    <div className={`activity-row flex min-w-0 items-start gap-2.5 text-sm ${run.status}`}>
      <RunStatusSquare status={run.status} className="mt-px" />
      <span className="flex min-w-0 flex-col gap-0.5">
        <strong
          className={cn(
            'font-medium break-words',
            run.status === 'failed' ? 'text-danger' : 'text-content',
          )}
        >
          {run.status === 'completed'
            ? run.summary || t('workflows:workflowsPage.workflowCompleted')
            : run.status === 'waiting_approval'
              ? t('workflows:workflowsPage.waitingApproval')
              : run.status === 'running'
                ? run.currentNodeLabel || t('workflows:workflowsPage.running')
                : run.error
                  ? workflowErrorMessage(run.error)
                  : t('workflows:workflowsPage.workflowFailed')}
        </strong>
        <small className="text-xs text-content-muted tabular-nums">
          {relativeTime(run.startedAt, language)} · {durationLabel(run.durationMs)}
        </small>
      </span>
    </div>
  )
}
