// 计划看板：把会话的计划项渲染成可勾选的进度面板，
// 点击切换状态并通过 update_plan 事件写回。
import { memo, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import {
  Check,
  ChevronDown,
  CircleDot,
  ListChecks,
  Lock,
  User,
  type LucideIcon,
} from 'lucide-react'
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
    <span
      className={`plan-board-status [&.completed]:text-[var(--success)] [&.running]:text-[var(--star-strong)] inline-flex items-center gap-[4px] flex-none text-[11px] text-[var(--text-muted)] ${STATUS_TONE[String(status)] || 'pending'}`}
    >
      <Icon size={12} />
      {label}
    </span>
  )
}

function PlanRow({ item, current, t }: { item: PlanItemView; current: boolean; t: Translate }) {
  return (
    <li
      className={`plan-board-row flex flex-col gap-[2px] [padding:5px_6px] rounded-[var(--r-sm)] ${item.blocked ? 'blocked [.plan-board-summary_&]:text-[var(--star-strong)] [.plan-board-row&]:opacity-[.72] [.plan-board-status&]:text-[var(--danger)]' : ''}`}
      aria-current={current ? 'step' : undefined}
      data-pisper-plan-current={current ? 'true' : undefined}
      data-pisper-plan-id={item.id}
    >
      <div className="flex items-center gap-[8px] min-w-0">
        <StatusBadge status={item.blocked ? 'blocked' : item.status} t={t} />
        <span
          className="flex-1 min-w-0 overflow-hidden text-[var(--text)] text-[13px] text-ellipsis whitespace-nowrap"
          title={item.title}
        >
          {item.title}
        </span>
        {item.assignee ? (
          <span
            className="inline-flex items-center gap-[4px] flex-none [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-pill)] bg-[var(--stroke-soft)] [padding:1px_8px] text-[var(--text-secondary)] text-[11px] whitespace-nowrap"
            title={t('chat:planBoard.assignedTo', { name: item.assignee })}
          >
            <User size={11} />
            {item.assignee}
          </span>
        ) : (
          item.status !== 'completed' && (
            <span className="flex-none text-[var(--text-muted)] text-[11px] [font-style:italic]">
              {t('chat:planBoard.unassigned')}
            </span>
          )
        )}
      </div>
      {item.blockedBy.length > 0 && (
        <div className="inline-flex items-center gap-[5px] [padding-left:2px] text-[var(--text-muted)] text-[11px]">
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
  collapsible?: boolean
}

/** Shared execution plan with ownership and dependency blockers. */
function PlanBoard({ plan, header, collapsible = false }: PlanBoardProps) {
  const { t } = useI18n()
  const listRef = useRef<HTMLUListElement>(null)
  const [expanded, setExpanded] = useState(true)
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
    if (collapsible && !expanded) return
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
  }, [collapsible, currentItemId, expanded, views.length])

  if (!items.length) return null

  const currentItem = views.find((view) => view.id === currentItemId)
  const summary = (
    <>
      <ListChecks className="flex-none text-[var(--star-strong)]" size={15} />
      <strong>{t('chat:planBoard.progress', { completed, total: items.length })}</strong>
      {currentItem ? (
        <span
          className="min-w-0 flex-1 overflow-hidden text-[var(--text-secondary)] text-ellipsis whitespace-nowrap"
          title={currentItem.title}
        >
          {currentItem.title}
        </span>
      ) : null}
      <span className="plan-board-counts @max-[470px]:hidden inline-flex flex-none items-center gap-[8px]">
        {inProgress > 0 && (
          <span>{t('chat:planBoard.countInProgress', { count: inProgress })}</span>
        )}
        {blockedCount > 0 && (
          <span className="blocked [.plan-board-summary_&]:text-[var(--star-strong)] [.plan-board-row&]:opacity-[.72] [.plan-board-status&]:text-[var(--danger)]">
            {t('chat:planBoard.countBlocked', { count: blockedCount })}
          </span>
        )}
      </span>
      {header}
      {collapsible ? (
        <ChevronDown
          className="plan-board-chevron [.plan-board-collapsible[open]_&]:[transform:rotate(180deg)] flex-none [transition:transform_var(--d1)_var(--ease-out)]"
          size={15}
        />
      ) : null}
    </>
  )
  const list = (
    <ul
      ref={listRef}
      className={`plan-board-list [.plan-board-collapsible_>_&]:[border-top:1px_solid_var(--stroke-soft)] [.plan-board-collapsible_>_&]:p-[8px_10px_10px] flex flex-col gap-[2px] m-0 [padding:8px_0_0] [list-style:none]${views.length > 4 ? ' is-scrollable [.session-plan-popover_.plan-board-list&]:[block-size:min(250px,38dvh)] [.plan-board-list&]:[block-size:min(220px,34dvh)] [.plan-board-list&]:overflow-y-auto [.plan-board-list&]:[overscroll-behavior:contain] [.plan-board-list&]:pr-[4px] [.plan-board-list&]:[scrollbar-gutter:stable] [.plan-board-list&:focus-visible]:[outline:2px_solid_var(--accent-border)] [.plan-board-list&:focus-visible]:[outline-offset:2px]' : ''}`}
      tabIndex={views.length > 4 ? 0 : undefined}
    >
      {views.map((item) => (
        <PlanRow key={item.id} item={item} current={item.id === currentItemId} t={t} />
      ))}
    </ul>
  )

  if (collapsible) {
    return (
      <details
        className="plan-board flex flex-col w-[min(900px,100%)] m-[0_auto] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--main-surface-bg)] p-[10px_14px] [.session-plan-popover_&]:w-full [.session-plan-popover_&]:border-0 [.session-plan-popover_&]:bg-transparent [.session-plan-popover_&]:p-0 plan-board-collapsible !p-0"
        open={expanded}
        onToggle={(event) => setExpanded(event.currentTarget.open)}
      >
        <summary
          className="plan-board-summary [&_strong]:flex-none [&_strong]:text-[var(--text)] [&_strong]:font-[600] [.plan-board-collapsible_>_&]:min-h-[38px] [.plan-board-collapsible_>_&]:border-0 [.plan-board-collapsible_>_&]:p-[7px_10px] [.plan-board-collapsible_>_&]:cursor-pointer [.plan-board-collapsible_>_&]:[list-style:none] [.plan-board-collapsible_>_&::-webkit-details-marker]:hidden [.plan-board-collapsible_>_&:hover]:bg-[var(--surface-hover)] [.plan-board-collapsible_>_&:focus-visible]:[outline:2px_solid_var(--accent-border)] [.plan-board-collapsible_>_&:focus-visible]:[outline-offset:-2px] @max-[470px]:[.plan-board-collapsible_>_&]:[padding-inline:8px] flex min-w-0 items-center gap-[8px] [padding-bottom:8px] [border-bottom:1px_solid_var(--stroke-soft)] text-[12px] text-[var(--text-secondary)]"
          aria-label={t('chat:planBoard.toggleAriaLabel')}
        >
          {summary}
        </summary>
        {list}
      </details>
    )
  }

  return (
    <section
      className="plan-board flex flex-col w-[min(900px,100%)] m-[0_auto] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--main-surface-bg)] p-[10px_14px] [.session-plan-popover_&]:w-full [.session-plan-popover_&]:border-0 [.session-plan-popover_&]:bg-transparent [.session-plan-popover_&]:p-0"
      aria-label={t('chat:planBoard.ariaLabel')}
    >
      <div className="plan-board-summary [&_strong]:flex-none [&_strong]:text-[var(--text)] [&_strong]:font-[600] [.plan-board-collapsible_>_&]:min-h-[38px] [.plan-board-collapsible_>_&]:border-0 [.plan-board-collapsible_>_&]:p-[7px_10px] [.plan-board-collapsible_>_&]:cursor-pointer [.plan-board-collapsible_>_&]:[list-style:none] [.plan-board-collapsible_>_&::-webkit-details-marker]:hidden [.plan-board-collapsible_>_&:hover]:bg-[var(--surface-hover)] [.plan-board-collapsible_>_&:focus-visible]:[outline:2px_solid_var(--accent-border)] [.plan-board-collapsible_>_&:focus-visible]:[outline-offset:-2px] @max-[470px]:[.plan-board-collapsible_>_&]:[padding-inline:8px] flex min-w-0 items-center gap-[8px] [padding-bottom:8px] [border-bottom:1px_solid_var(--stroke-soft)] text-[12px] text-[var(--text-secondary)]">
        {summary}
      </div>
      {list}
    </section>
  )
}

export default memo(PlanBoard)
