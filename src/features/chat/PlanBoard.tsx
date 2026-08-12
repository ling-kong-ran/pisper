import { memo, useLayoutEffect, useRef, type ReactNode } from 'react'
import { Check, CircleDot, Lock, User, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { I18nValues } from '@/app/i18n'
import type { Plan, PlanItem } from '@/types/chat'

type Translate = (message: string, values?: I18nValues) => string

type PlanItemView = PlanItem & {
  id: string
  blocked: boolean
  blockedBy: PlanItem[]
}

const STATUS_TONE: Record<string, string> = {
  completed: 'completed',
  in_progress: 'running',
  blocked: 'blocked',
  pending: 'pending',
}

function buildPlanItemViews(items: PlanItem[]): PlanItemView[] {
  const byId = new Map(items.map((item) => [String(item.id || ''), item]))
  const isDone = (item: PlanItem | undefined) => item?.status === 'completed'
  return items.map((item) => {
    const id = String(item.id || '')
    const dependsOn = Array.isArray(item.dependsOn) ? item.dependsOn : []
    const blockedBy = dependsOn
      .map((depId) => byId.get(String(depId)))
      .filter((dep): dep is PlanItem => Boolean(dep) && !isDone(dep))
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
      ? t('chat:planBoard.completed')
      : status === 'in_progress'
        ? t('chat:planBoard.inProgress')
        : status === 'blocked'
          ? t('chat:planBoard.blocked')
          : t('chat:planBoard.pending')
  return (
    <span className={`plan-board-status ${STATUS_TONE[String(status)] || 'pending'}`}>
      <Icon size={12} />
      {label}
    </span>
  )
}

function PlanRow({ item, current, t }: { item: PlanItemView; current: boolean; t: Translate }) {
  return (
    <li
      className={`plan-board-row ${item.blocked ? 'blocked' : ''}`}
      aria-current={current ? 'step' : undefined}
      data-pisper-plan-current={current ? 'true' : undefined}
      data-pisper-plan-id={item.id}
    >
      <div className="plan-board-row-head">
        <StatusBadge status={item.blocked ? 'blocked' : item.status} t={t} />
        <span className="plan-board-title" title={item.title}>
          {item.title}
        </span>
        {item.assignee ? (
          <span
            className="plan-board-assignee"
            title={t('chat:planBoard.assignedTo', { name: item.assignee })}
          >
            <User size={11} />
            {item.assignee}
          </span>
        ) : (
          item.status !== 'completed' && (
            <span className="plan-board-unassigned">{t('chat:planBoard.unassigned')}</span>
          )
        )}
      </div>
      {item.blockedBy.length > 0 && (
        <div className="plan-board-deps">
          <Lock size={11} />
          {t('chat:planBoard.waitingFor', {
            tasks: item.blockedBy.map((dep) => dep.title || dep.id).join(' · '),
          })}
        </div>
      )}
    </li>
  )
}

export type PlanBoardProps = {
  plan: Plan | null
  header?: ReactNode
}

/** Shared execution plan with ownership and dependency blockers. */
function PlanBoard({ plan, header }: PlanBoardProps) {
  const { t } = useI18n()
  const listRef = useRef<HTMLUListElement>(null)
  const items = plan?.items || []
  const views = buildPlanItemViews(items)
  const completed = views.filter((view) => view.status === 'completed').length
  const inProgress = views.filter((view) => view.status === 'in_progress' && !view.blocked).length
  const blockedCount = views.filter((view) => view.blocked).length
  const currentItemId =
    views.find((view) => view.status === 'in_progress' && !view.blocked)?.id ??
    views.find((view) => view.blocked)?.id ??
    views.find((view) => view.status === 'pending')?.id ??
    views.at(-1)?.id

  useLayoutEffect(() => {
    const list = listRef.current
    const current = list?.querySelector<HTMLElement>('[data-pisper-plan-current="true"]')
    if (!list || !current) return

    const listBounds = list.getBoundingClientRect()
    const currentBounds = current.getBoundingClientRect()
    if (currentBounds.top < listBounds.top) {
      list.scrollTop -= listBounds.top - currentBounds.top
    } else if (currentBounds.bottom > listBounds.bottom) {
      list.scrollTop += currentBounds.bottom - listBounds.bottom
    }
  }, [currentItemId, views.length])

  if (!items.length) return null

  return (
    <section className="plan-board" aria-label={t('chat:planBoard.ariaLabel')}>
      <div className="plan-board-summary">
        <strong>{t('chat:planBoard.progress', { completed, total: items.length })}</strong>
        {inProgress > 0 && (
          <span>{t('chat:planBoard.countInProgress', { count: inProgress })}</span>
        )}
        {blockedCount > 0 && (
          <span className="blocked">
            {t('chat:planBoard.countBlocked', { count: blockedCount })}
          </span>
        )}
        {header}
      </div>
      <ul
        ref={listRef}
        className={`plan-board-list${views.length > 4 ? ' is-scrollable' : ''}`}
        tabIndex={views.length > 4 ? 0 : undefined}
      >
        {views.map((item) => (
          <PlanRow key={item.id} item={item} current={item.id === currentItemId} t={t} />
        ))}
      </ul>
    </section>
  )
}

export default memo(PlanBoard)
