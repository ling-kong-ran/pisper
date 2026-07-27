import { memo, type ReactNode } from 'react'
import { Check, CircleDot, Lock, User, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { I18nValues } from '@/app/i18n'
import type { TaskList, TaskListItem } from '@/types/chat'

type Translate = (message: string, values?: I18nValues) => string

type TaskView = TaskListItem & {
  id: string
  blocked: boolean
  blockedBy: TaskListItem[]
}

const STATUS_TONE: Record<string, string> = {
  completed: 'completed',
  in_progress: 'running',
  blocked: 'blocked',
  pending: 'pending',
}

function buildTaskViews(items: TaskListItem[]): TaskView[] {
  const byId = new Map(items.map((item) => [String(item.id || ''), item]))
  const isDone = (item: TaskListItem | undefined) => item?.status === 'completed'
  return items.map((item) => {
    const id = String(item.id || '')
    const dependsOn = Array.isArray(item.dependsOn) ? item.dependsOn : []
    const blockedBy = dependsOn
      .map((depId) => byId.get(String(depId)))
      .filter((dep): dep is TaskListItem => Boolean(dep) && !isDone(dep))
    const blocked =
      item.status !== 'completed' && (item.status === 'blocked' || blockedBy.length > 0)
    return { ...item, id, blocked, blockedBy }
  })
}

function statusIcon(status: string | undefined): LucideIcon {
  if (status === 'completed') return Check
  if (status === 'in_progress') return CircleDot
  if (status === 'blocked') return Lock
  return CircleDot
}

function StatusBadge({ status, t }: { status: string | undefined; t: Translate }) {
  const Icon = statusIcon(status)
  const label =
    status === 'completed'
      ? t('chat:taskBoard.completed')
      : status === 'in_progress'
        ? t('chat:taskBoard.inProgress')
        : status === 'blocked'
          ? t('chat:taskBoard.blocked')
          : t('chat:taskBoard.pending')
  return (
    <span className={`task-board-status ${STATUS_TONE[String(status)] || 'pending'}`}>
      <Icon size={12} />
      {label}
    </span>
  )
}

function TaskRow({ task, t }: { task: TaskView; t: Translate }) {
  return (
    <li className={`task-board-row ${task.blocked ? 'blocked' : ''}`} data-pisper-task-id={task.id}>
      <div className="task-board-row-head">
        <StatusBadge status={task.blocked ? 'blocked' : task.status} t={t} />
        <span className="task-board-title" title={task.title}>
          {task.title}
        </span>
        {task.assignee ? (
          <span
            className="task-board-assignee"
            title={t('chat:taskBoard.assignedTo', { name: task.assignee })}
          >
            <User size={11} />
            {task.assignee}
          </span>
        ) : (
          task.status !== 'completed' && (
            <span className="task-board-unassigned">{t('chat:taskBoard.unassigned')}</span>
          )
        )}
      </div>
      {task.blockedBy.length > 0 && (
        <div className="task-board-deps">
          <Lock size={11} />
          {t('chat:taskBoard.waitingFor', {
            tasks: task.blockedBy.map((dep) => dep.title || dep.id).join(' · '),
          })}
        </div>
      )}
    </li>
  )
}

export type TaskBoardProps = {
  taskList: TaskList | null
  header?: ReactNode
}

/**
 * Team-wide task board: renders the shared task list with assignee and
 * dependency chains so progress, ownership, and blockers are visible at a glance.
 */
function TaskBoard({ taskList, header }: TaskBoardProps) {
  const { t } = useI18n()
  const items = taskList?.items || []
  if (!items.length) return null
  const views = buildTaskViews(items)
  const completed = views.filter((view) => view.status === 'completed').length
  const inProgress = views.filter((view) => view.status === 'in_progress' && !view.blocked).length
  const blockedCount = views.filter((view) => view.blocked).length

  return (
    <section className="task-board" aria-label={t('chat:taskBoard.ariaLabel')}>
      <div className="task-board-summary">
        <strong>{t('chat:taskBoard.progress', { completed, total: items.length })}</strong>
        {inProgress > 0 && (
          <span>{t('chat:taskBoard.countInProgress', { count: inProgress })}</span>
        )}
        {blockedCount > 0 && (
          <span className="blocked">
            {t('chat:taskBoard.countBlocked', { count: blockedCount })}
          </span>
        )}
        {header}
      </div>
      <ul className="task-board-list">
        {views.map((task) => (
          <TaskRow key={task.id} task={task} t={t} />
        ))}
      </ul>
    </section>
  )
}

export default memo(TaskBoard)
