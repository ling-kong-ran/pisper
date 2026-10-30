// 手机看板（phoneShell）：56px 页头（“看板” 24/650 + 副标题，右侧 44px 搜索与筛选）、
// 40px 泳道跳转条（需要你选中时琥珀实心），以及纵向堆叠、标题吸顶的泳道区段。
// 卡片圆角 12（--r-card 在手机外壳下放大），审批卡的拒绝 / 允许是各占一半的 44px 大按钮。
// 新任务走底部标签栏中间的按钮 → 派发底部表单；空看板时直接在页内放一个启动台。
// 筛选菜单底部放“新建项目…”“管理项目…”：手机没有页头的“项目 ▾”，项目入口并在这里。
import { useEffect, useRef, useState, type RefObject } from 'react'
import { ArrowRight, FolderCog, FolderPlus, ListFilter, Search } from 'lucide-react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { requestCommandPalette } from '@/features/chat/events'
import { cn } from '@/lib/utils'
import { AgentCard } from './AgentCard'
import { ApprovalConfirm } from './ApprovalConfirm'
import type { BoardHeaderProject } from './BoardHeader'
import { useBoardViewStore } from './board-view-store'
import { LaneGlyph } from './LaneGlyph'
import { useLaneLabels } from './lane-labels'
import { visibleLaneItems } from './lane-layout'
import { LaunchConsole } from './LaunchConsole'
import type { useBoardController } from './useBoardController'
import type { BoardSessionsResult } from './useBoardSessions'

type PhoneBoardProps = {
  rootRef: RefObject<HTMLDivElement | null>
  board: BoardSessionsResult
  availableLanes: readonly BoardLane[]
  projects: BoardHeaderProject[]
  now: number
  controller: ReturnType<typeof useBoardController>
  announcement: string
  onConfigureModels: () => void
  onOpenHistory: () => void
  projectPicking: boolean
  onNewProject: () => void
  onManageProjects: () => void
}

const LANE_TONES: Record<BoardLane, string> = {
  attention: 'text-st-attn',
  running: 'text-st-run',
  queued: 'text-st-queue',
  done: 'text-st-done',
}

// 跳转条选中态：需要你用实心琥珀（--st-attn-solid + 内描边），其余泳道用浅底 + 描边，
// 因为只有琥珀定义了实心底上的文字色（--on-attn）。
const JUMP_SELECTED: Record<BoardLane, string> = {
  attention: 'border-transparent bg-st-attn-solid text-on-attn ring-1 ring-st-attn-ring ring-inset',
  running: 'border-st-run-border bg-st-run-soft text-st-run',
  queued: 'border-st-queue-border bg-st-queue-soft text-st-queue',
  done: 'border-st-done-border bg-st-done-soft text-st-done',
}

export function PhoneBoard({
  rootRef,
  board,
  availableLanes,
  projects,
  now,
  controller,
  announcement,
  onConfigureModels,
  onOpenHistory,
  projectPicking,
  onNewProject,
  onManageProjects,
}: PhoneBoardProps) {
  const { t } = useI18n()
  const labels = useLaneLabels()
  const view = useBoardViewStore()
  const scrollerRef = useRef<HTMLDivElement>(null)
  const lanes = availableLanes.filter((lane) => !view.lanes.length || view.lanes.includes(lane))
  const [current, setCurrent] = useState<BoardLane>(lanes[0] ?? 'attention')
  // 最近一次在跳转条上点的泳道；用户自己拖动列表后作废。
  const jumpTargetRef = useRef<BoardLane | null>(null)
  const lanesKey = lanes.join(',')

  // 跳转条跟随滚动：标题已经滚过顶部的最后一个区段就是当前区段。
  useEffect(() => {
    const scroller = scrollerRef.current
    if (!scroller) return
    let frame = 0
    const update = () => {
      const top = scroller.getBoundingClientRect().top
      const sections = [...scroller.querySelectorAll<HTMLElement>('[data-board-section]')]
      let active: BoardLane | null = null
      for (const section of sections)
        if (section.getBoundingClientRect().top - top <= 12)
          active = section.dataset.boardSection as BoardLane
      // 滚到底时，末尾较短的区段（常见是“已完成”）到不了顶部：不能让前一个区段抢走选中，
      // 否则点了“已完成”却点亮“运行中”。到底后以露出标题的最后一个区段为准，刚点过的泳道优先。
      const atBottom =
        scroller.scrollTop > 0 &&
        scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2
      if (atBottom) {
        const visible = sections
          .filter((section) => section.getBoundingClientRect().top - top < scroller.clientHeight)
          .map((section) => section.dataset.boardSection as BoardLane)
        const target = jumpTargetRef.current
        active = (target && visible.includes(target) ? target : visible.at(-1)) ?? active
      }
      setCurrent((value) => active ?? (lanesKey.split(',')[0] as BoardLane | undefined) ?? value)
    }
    const schedule = () => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(update)
    }
    const release = () => {
      jumpTargetRef.current = null
    }
    update()
    scroller.addEventListener('scroll', schedule, { passive: true })
    scroller.addEventListener('pointerdown', release, { passive: true })
    scroller.addEventListener('wheel', release, { passive: true })
    return () => {
      cancelAnimationFrame(frame)
      scroller.removeEventListener('scroll', schedule)
      scroller.removeEventListener('pointerdown', release)
      scroller.removeEventListener('wheel', release)
    }
    // 区段由筛选与内容决定：二者变化时重新计算一次当前区段即可。
  }, [lanesKey, board.items.length])

  const jump = (lane: BoardLane) => {
    const scroller = scrollerRef.current
    const section = scroller?.querySelector<HTMLElement>(`[data-board-section="${lane}"]`)
    if (!scroller || !section) return
    jumpTargetRef.current = lane
    scroller.scrollTo({ top: section.offsetTop, behavior: 'smooth' })
    setCurrent(lane)
  }

  let content
  if (board.isLoading) content = <PageState kind="loading" title={t('board:page.loading')} />
  else if (board.error && board.total === 0)
    content = (
      <PageState
        kind="error"
        title={t('board:page.error')}
        description={t('board:page.errorHint')}
        action={
          <Button variant="secondary" className="min-h-11" onClick={() => void board.refresh()}>
            {t('board:page.retry')}
          </Button>
        }
      />
    )
  else if (board.total === 0 && board.catalogEmpty)
    // 首次运行：页内直接放一个启动台。只是今天没有活动时照常显示泳道区段（已完成区段给出切到 7 天）。
    content = (
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 pt-6 pb-6">
        <h2 className="text-lg font-semibold text-content">{t('board:page.empty')}</h2>
        <p className="text-sm text-content-muted">{t('board:page.emptyHint')}</p>
        <LaunchConsole variant="sheet" onConfigureModels={onConfigureModels} />
      </div>
    )
  else
    content = (
      <>
        <nav
          aria-label={t('board:phone.jumpBar')}
          className="flex h-12 flex-none items-center gap-2 overflow-x-auto px-4 pb-2 [scrollbar-width:none]"
        >
          {lanes.map((lane) => {
            const selected = current === lane
            return (
              <button
                key={lane}
                type="button"
                aria-current={selected ? 'true' : undefined}
                onClick={() => jump(lane)}
                className={cn(
                  'flex h-10 flex-none items-center gap-1.5 rounded-sm border px-3 text-sm font-medium focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus',
                  selected
                    ? JUMP_SELECTED[lane]
                    : 'border-stroke-strong bg-solid text-content-secondary',
                )}
              >
                <LaneGlyph lane={lane} className={cn('size-4', !selected && LANE_TONES[lane])} />
                {labels.name[lane]}
                <span className="font-mono tabular-nums">{board.lanes[lane].length}</span>
              </button>
            )
          })}
        </nav>
        <div
          ref={scrollerRef}
          data-board-sections
          className="relative min-h-0 flex-1 overflow-y-auto overscroll-contain border-t border-stroke-soft px-4 pb-6"
        >
          {board.total > 0 && board.items.length === 0 && (
            <PageState
              kind="empty"
              title={t('board:filter.emptyTitle')}
              description={t('board:filter.emptyHint')}
              action={
                <Button variant="secondary" className="min-h-11" onClick={view.clearFilters}>
                  {t('board:filter.clear')}
                </Button>
              }
            />
          )}
          {(board.items.length > 0 || board.total === 0) &&
            lanes.map((lane) => {
              const items = visibleLaneItems(lane, board.lanes[lane])
              const headingId = `board-section-${lane}`
              return (
                <section
                  key={lane}
                  data-board-section={lane}
                  aria-labelledby={headingId}
                  className="pt-1"
                >
                  <div className="sticky top-0 z-10 -mx-4 bg-background px-4 pt-3 pb-2">
                    <h2
                      id={headingId}
                      className="flex items-center gap-2 text-base font-semibold text-content"
                    >
                      <LaneGlyph lane={lane} className={cn('size-4', LANE_TONES[lane])} />
                      <span className={cn(lane === 'attention' && 'text-st-attn')}>
                        {labels.name[lane]}
                      </span>
                      <span
                        className={cn(
                          'rounded-xs border px-1.5 font-mono text-xs leading-5 tabular-nums',
                          lane === 'attention'
                            ? 'border-st-attn-border bg-st-attn-soft text-st-attn'
                            : 'border-stroke-soft bg-surface-muted text-content-secondary',
                        )}
                      >
                        {board.lanes[lane].length}
                      </span>
                      <span className="ml-auto text-xs font-normal text-content-muted">
                        {labels.order[lane]}
                      </span>
                    </h2>
                    {lane === 'attention' && (
                      <span
                        aria-hidden="true"
                        className="mt-2 block h-1 rounded-full bg-[image:var(--hatch-attn)]"
                      />
                    )}
                  </div>
                  {items.length === 0 ? (
                    lane === 'done' ? (
                      <div className="flex flex-wrap items-center gap-2 py-3 text-sm text-content-muted">
                        <p>
                          {view.doneWindow === 'today'
                            ? t('board:lane.doneEmptyToday')
                            : t('board:lane.doneEmptyWeek')}
                        </p>
                        {view.doneWindow === 'today' && (
                          <Button
                            variant="outline"
                            className="min-h-11"
                            onClick={() => view.setDoneWindow('7d')}
                          >
                            {t('board:lane.switchToWeek')}
                          </Button>
                        )}
                      </div>
                    ) : (
                      <p className="py-3 text-sm text-content-muted">
                        {lane === 'attention'
                          ? t('board:lane.attentionEmpty')
                          : t('board:lane.empty')}
                      </p>
                    )
                  ) : (
                    <div className="flex flex-col gap-3 pb-3">
                      {items.map((item) => (
                        <AgentCard
                          key={item.id}
                          item={item}
                          now={now}
                          phone
                          active={item.id === controller.activeId}
                          handlers={controller.handlers}
                        />
                      ))}
                    </div>
                  )}
                  {lane === 'done' && (
                    <Button
                      variant="ghost"
                      className="mb-3 min-h-11 w-full text-content-secondary"
                      onClick={onOpenHistory}
                    >
                      {t('board:lane.history')}
                      <ArrowRight aria-hidden="true" />
                    </Button>
                  )}
                </section>
              )
            })}
        </div>
      </>
    )

  return (
    <div
      ref={rootRef}
      data-board-root
      className="relative flex h-full min-h-0 w-full min-w-0 flex-col bg-background"
    >
      <header className="flex h-14 flex-none items-center gap-2 pr-2 pl-4">
        <h1 className="text-2xl leading-8 font-[650] tracking-[-.015em] text-content">
          {t('board:page.title')}
        </h1>
        {board.total > 0 && (
          <p className="min-w-0 truncate text-xs text-content-muted">
            {t('board:phone.summary', { agents: board.total, running: board.lanes.running.length })}
          </p>
        )}
        <span className="min-w-0 flex-1" />
        <Button
          variant="ghost"
          size="icon-lg"
          className="size-11"
          aria-label={t('board:phone.search')}
          onClick={requestCommandPalette}
        >
          <Search className="size-5" aria-hidden="true" />
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-lg"
              className={cn(
                'size-11',
                (view.lanes.length > 0 || view.projects.length > 0) && 'text-content',
              )}
              aria-label={t('board:phone.filter')}
            >
              <ListFilter className="size-5" aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="max-h-[70dvh] w-64 overflow-y-auto">
            <DropdownMenuLabel>{t('board:filter.statuses')}</DropdownMenuLabel>
            {availableLanes.map((lane) => (
              <DropdownMenuCheckboxItem
                key={lane}
                className="min-h-11"
                checked={view.lanes.includes(lane)}
                onCheckedChange={() => view.toggleLaneFilter(lane)}
              >
                {labels.name[lane]}
              </DropdownMenuCheckboxItem>
            ))}
            {projects.length > 0 && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuLabel>{t('board:filter.projects')}</DropdownMenuLabel>
                {projects.map((project) => (
                  <DropdownMenuCheckboxItem
                    key={project.key}
                    className="min-h-11"
                    checked={view.projects.includes(project.key)}
                    onCheckedChange={() => view.toggleProjectFilter(project.key)}
                  >
                    <span className="min-w-0 truncate font-mono text-xs">{project.label}</span>
                    <span className="ml-auto font-mono text-xs text-content-muted tabular-nums">
                      {project.count}
                    </span>
                  </DropdownMenuCheckboxItem>
                ))}
              </>
            )}
            <DropdownMenuSeparator />
            <DropdownMenuItem
              className="min-h-11"
              disabled={projectPicking}
              onSelect={onNewProject}
            >
              <FolderPlus aria-hidden="true" />
              {t('board:project.new')}
            </DropdownMenuItem>
            <DropdownMenuItem className="min-h-11" onSelect={onManageProjects}>
              <FolderCog aria-hidden="true" />
              {t('board:project.manage')}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </header>
      {content}
      <p aria-live="polite" role="status" className="sr-only">
        {announcement}
      </p>
      <ApprovalConfirm
        target={controller.confirm?.target ?? null}
        phone
        pending={controller.confirm?.pending ?? false}
        error={controller.confirm?.error ?? ''}
        onConfirm={() => void controller.confirmApproval()}
        onOpenChange={controller.setConfirmOpen}
      />
    </div>
  )
}
