// 计划看板：把会话的计划项渲染成可勾选的进度面板，
// 点击切换状态并通过 update_plan 事件写回。
import { memo, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { Check, ChevronDown, Circle, ListChecks, Lock, User } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { I18nValues } from '@/app/i18n'
import { OrbitSpinner } from '@/components/ui/spinner'
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

// 状态方块与活动日志同一语言：形状（勾/轨道/锁/空圈）+ 颜色共同表达状态，文字标签留给读屏。
function StatusBadge({ status, t }: { status: string | undefined; t: Translate }) {
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
      className={`plan-board-status grid size-[18px] flex-none place-items-center rounded-xs text-content-muted [&.completed]:bg-success-soft [&.completed]:text-success [&.running]:text-accent-fg [&.blocked]:bg-danger-soft [&.blocked]:text-danger ${STATUS_TONE[String(status)] || 'pending'}`}
      title={label}
    >
      {status === 'completed' ? (
        <Check size={12} strokeWidth={2.25} />
      ) : status === 'in_progress' ? (
        <OrbitSpinner className="size-3.5" />
      ) : status === 'blocked' ? (
        <Lock size={11} strokeWidth={2} />
      ) : (
        <Circle size={11} strokeWidth={2} />
      )}
      <span className="sr-only">{label}</span>
    </span>
  )
}

function PlanRow({ item, current, t }: { item: PlanItemView; current: boolean; t: Translate }) {
  return (
    <li
      className={`plan-board-row flex min-w-0 flex-col gap-[2px] rounded-sm px-[6px] py-[6px] aria-[current=step]:bg-surface-subtle ${item.blocked ? 'blocked' : ''}`}
      aria-current={current ? 'step' : undefined}
      data-pisper-plan-current={current ? 'true' : undefined}
      data-pisper-plan-id={item.id}
    >
      <div className="flex min-w-0 items-center gap-[10px]">
        <StatusBadge status={item.blocked ? 'blocked' : item.status} t={t} />
        <span
          className={`min-w-0 flex-1 truncate text-[13px] ${item.status === 'completed' ? 'text-content-muted' : 'text-foreground'}`}
          title={item.title}
        >
          {item.title}
        </span>
        {item.assignee ? (
          <span
            className="inline-flex max-w-[40%] flex-none items-center gap-[4px] truncate rounded-xs border border-stroke px-[6px] font-mono text-[12px] leading-[18px] text-content-secondary"
            title={t('chat:planBoard.assignedTo', { name: item.assignee })}
          >
            <User size={11} className="flex-none" />
            {item.assignee}
          </span>
        ) : (
          item.status !== 'completed' && (
            <span className="flex-none text-[12px] text-content-muted">
              {t('chat:planBoard.unassigned')}
            </span>
          )
        )}
      </div>
      {item.blockedBy.length > 0 && (
        <div className="inline-flex min-w-0 items-center gap-[5px] pl-[28px] text-[12px] text-content-muted">
          <Lock size={11} className="flex-none" />
          <span className="truncate">
            {t('chat:planBoard.waitingFor', {
              tasks: item.blockedBy.map((dep) => dep.title || dep.id).join(' · '),
            })}
          </span>
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
      <ListChecks className="flex-none text-content-muted" size={15} />
      <strong className="tabular-nums">
        {t('chat:planBoard.progress', { completed, total: items.length })}
      </strong>
      {currentItem ? (
        <span className="min-w-0 flex-1 truncate text-content-secondary" title={currentItem.title}>
          {currentItem.title}
        </span>
      ) : null}
      <span className="plan-board-counts inline-flex flex-none items-center gap-[8px] text-content-muted tabular-nums @max-[470px]:hidden">
        {inProgress > 0 && (
          <span>{t('chat:planBoard.countInProgress', { count: inProgress })}</span>
        )}
        {blockedCount > 0 && (
          <span className="blocked text-danger">
            {t('chat:planBoard.countBlocked', { count: blockedCount })}
          </span>
        )}
      </span>
      {header}
      {collapsible ? (
        <ChevronDown
          className="plan-board-chevron flex-none text-content-muted transition-transform duration-(--d-fast) ease-precise [.plan-board-collapsible[open]_&]:rotate-180"
          size={15}
        />
      ) : null}
    </>
  )
  const list = (
    <ul
      ref={listRef}
      className={`plan-board-list [.plan-board-collapsible_>_&]:border-t [.plan-board-collapsible_>_&]:border-stroke-soft [.plan-board-collapsible_>_&]:p-[6px_8px_8px] m-0 flex list-none flex-col gap-[1px] pt-[6px]${views.length > 4 ? ' is-scrollable [.session-plan-popover_.plan-board-list&]:[block-size:min(250px,38dvh)] [.plan-board-list&]:[block-size:min(220px,34dvh)] [.plan-board-list&]:overflow-y-auto [.plan-board-list&]:[overscroll-behavior:contain] [.plan-board-list&]:pr-[4px] [.plan-board-list&]:[scrollbar-gutter:stable] [.plan-board-list&:focus-visible]:outline-2 [.plan-board-list&:focus-visible]:outline-focus [.plan-board-list&:focus-visible]:outline-offset-2' : ''}`}
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
        className="plan-board plan-board-collapsible m-[0_auto] flex w-[min(900px,100%)] flex-col overflow-hidden rounded-md border border-stroke bg-panel !p-0 [.session-plan-popover_&]:w-full [.session-plan-popover_&]:border-0 [.session-plan-popover_&]:bg-transparent"
        open={expanded}
        onToggle={(event) => setExpanded(event.currentTarget.open)}
      >
        <summary
          className="plan-board-summary flex min-w-0 items-center gap-[8px] border-b border-stroke-soft pb-[8px] text-[13px] text-content-secondary [&_strong]:flex-none [&_strong]:font-medium [&_strong]:text-foreground [.plan-board-collapsible_>_&]:min-h-[34px] [.plan-board-collapsible_>_&]:cursor-pointer [.plan-board-collapsible_>_&]:list-none [.plan-board-collapsible_>_&]:border-0 [.plan-board-collapsible_>_&]:p-[6px_12px] max-[650px]:[.plan-board-collapsible_>_&]:min-h-11 [.plan-board-collapsible_>_&::-webkit-details-marker]:hidden [.plan-board-collapsible_>_&:hover]:bg-surface-subtle [.plan-board-collapsible_>_&:focus-visible]:outline-2 [.plan-board-collapsible_>_&:focus-visible]:-outline-offset-2 [.plan-board-collapsible_>_&:focus-visible]:outline-focus"
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
      className="plan-board m-[0_auto] flex w-[min(900px,100%)] flex-col rounded-md border border-stroke bg-panel p-[10px_12px] [.session-plan-popover_&]:w-full [.session-plan-popover_&]:border-0 [.session-plan-popover_&]:bg-transparent [.session-plan-popover_&]:p-0"
      aria-label={t('chat:planBoard.ariaLabel')}
    >
      <div className="plan-board-summary flex min-w-0 items-center gap-[8px] border-b border-stroke-soft pb-[8px] text-[13px] text-content-secondary [&_strong]:flex-none [&_strong]:font-medium [&_strong]:text-foreground [.plan-board-collapsible_>_&]:min-h-[34px] [.plan-board-collapsible_>_&]:cursor-pointer [.plan-board-collapsible_>_&]:list-none [.plan-board-collapsible_>_&]:border-0 [.plan-board-collapsible_>_&]:p-[6px_12px] max-[650px]:[.plan-board-collapsible_>_&]:min-h-11 [.plan-board-collapsible_>_&::-webkit-details-marker]:hidden [.plan-board-collapsible_>_&:hover]:bg-surface-subtle [.plan-board-collapsible_>_&:focus-visible]:outline-2 [.plan-board-collapsible_>_&:focus-visible]:-outline-offset-2 [.plan-board-collapsible_>_&:focus-visible]:outline-focus">
        {summary}
      </div>
      {list}
    </section>
  )
}

export default memo(PlanBoard)
