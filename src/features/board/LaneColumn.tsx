// 泳道：标题（字形 + 名称 + 计数 + 固定排序说明）、独立滚动的卡片列、底部吸附的溢出摘要。
// 空泳道与手动折叠的已完成泳道收成 56px 竖条；“需要你”为空时竖条写绿色“✓ 已清空”。
// 卡片超过 30 张改用虚拟列表；已完成最多 20 张，后面接“全部历史会话 →”。
import {
  useCallback,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type Ref,
} from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { ArrowDown, ArrowRight, Check, ChevronDown, PanelLeftClose } from 'lucide-react'
import type { BoardDoneWindow, BoardLane } from '@shared/session-board-status.mjs'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { AgentCard, type AgentCardHandlers } from './AgentCard'
import type { BoardItem } from './board-model'
import { LaneGlyph } from './LaneGlyph'
import { useLaneLabels } from './lane-labels'
import {
  hiddenBelowCount,
  laneShouldVirtualize,
  visibleLaneItems,
  type LaneLayoutEntry,
} from './lane-layout'

export type LaneHandle = {
  lane: BoardLane
  scroller: HTMLElement | null
  // 让某张卡片进入可视区（虚拟列表先滚到索引再等它渲染），返回卡片元素。
  reveal: (sessionId: string) => Promise<HTMLElement | null>
}

const LANE_GLYPH_TONES: Record<BoardLane, string> = {
  attention: 'text-st-attn',
  running: 'text-st-run',
  queued: 'text-st-queue',
  done: 'text-st-done',
}

function nextFrame() {
  return new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
}

function cardElement(root: HTMLElement | null, sessionId: string) {
  return root?.querySelector<HTMLElement>(`[data-board-card="${CSS.escape(sessionId)}"]`) ?? null
}

type LaneColumnProps = {
  entry: LaneLayoutEntry
  items: BoardItem[]
  now: number
  activeId: string
  dimmed: boolean
  handlers: AgentCardHandlers
  doneWindow: BoardDoneWindow
  onDoneWindowChange: (window: BoardDoneWindow) => void
  onPeek: (lane: BoardLane, peeked: boolean) => void
  onCollapse: (lane: BoardLane, collapsed: boolean) => void
  onOpenHistory: () => void
  // 首次运行（目录里一个会话都没有）时“切到 7 天”没有意义，不显示。
  offerWeek?: boolean
  laneRef?: Ref<LaneHandle>
}

export function LaneColumn({
  entry,
  items,
  now,
  activeId,
  dimmed,
  handlers,
  doneWindow,
  onDoneWindowChange,
  onPeek,
  onCollapse,
  onOpenHistory,
  offerWeek = true,
  laneRef,
}: LaneColumnProps) {
  const { t } = useI18n()
  const labels = useLaneLabels()
  const { lane } = entry
  const headingId = `board-lane-${lane}`
  const scrollerRef = useRef<HTMLDivElement>(null)
  const shown = visibleLaneItems(lane, items)
  const virtual = laneShouldVirtualize(shown.length)
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLElement>({
    count: virtual ? shown.length : 0,
    getScrollElement: () => scrollerRef.current,
    estimateSize: () => 132,
    gap: 8,
    overscan: 6,
    getItemKey: (index) => shown[index]?.id ?? index,
  })
  const [hidden, setHidden] = useState(0)

  // 溢出摘要：数一数可视区底部以下还有几张卡片；虚拟列表按索引估算。
  const measureOverflow = useCallback(() => {
    const scroller = scrollerRef.current
    if (!scroller) return
    if (virtual) {
      const last = virtualizer.getVirtualItems().at(-1)
      const visibleEnd = scroller.scrollTop + scroller.clientHeight
      const below = virtualizer.getVirtualItems().filter((item) => item.end > visibleEnd + 8).length
      setHidden(below + Math.max(0, shown.length - 1 - (last?.index ?? shown.length - 1)))
      return
    }
    const bottom = scroller.getBoundingClientRect().bottom
    const bottoms = [...scroller.querySelectorAll<HTMLElement>('[data-board-card]')].map(
      (card) => card.getBoundingClientRect().bottom,
    )
    setHidden(hiddenBelowCount(bottoms, bottom))
  }, [shown.length, virtual, virtualizer])

  useLayoutEffect(() => {
    if (entry.mode !== 'expanded') return
    measureOverflow()
    const scroller = scrollerRef.current
    if (!scroller) return
    let frame = 0
    const schedule = () => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(measureOverflow)
    }
    scroller.addEventListener('scroll', schedule, { passive: true })
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(schedule)
    observer?.observe(scroller)
    if (scroller.firstElementChild) observer?.observe(scroller.firstElementChild)
    return () => {
      cancelAnimationFrame(frame)
      scroller.removeEventListener('scroll', schedule)
      observer?.disconnect()
    }
  }, [entry.mode, measureOverflow, items])

  const reveal = useCallback(
    async (sessionId: string) => {
      const index = shown.findIndex((item) => item.id === sessionId)
      if (index < 0) return null
      if (virtual) {
        virtualizer.scrollToIndex(index, { align: 'auto' })
        await nextFrame()
        await nextFrame()
      }
      const card = cardElement(scrollerRef.current, sessionId)
      card?.scrollIntoView({ block: 'nearest' })
      return card
    },
    [shown, virtual, virtualizer],
  )

  useImperativeHandle(laneRef, () => ({ lane, scroller: scrollerRef.current, reveal }), [
    lane,
    reveal,
  ])

  // 竖条模式离开后，临时展开（peek）在泳道再次变空时自动收回由页面负责；这里只负责点击。
  if (entry.mode === 'rail') {
    const cleared = lane === 'attention' && entry.railReason === 'empty'
    const collapsedDone = entry.railReason === 'collapsed'
    return (
      <section
        aria-labelledby={headingId}
        data-board-lane={lane}
        data-lane-mode="rail"
        className={cn(
          'flex min-h-0 min-w-0 flex-col items-stretch rounded-lg border border-stroke-soft bg-lane transition-opacity duration-(--d-fast)',
          dimmed && 'opacity-50',
        )}
      >
        <button
          type="button"
          className="flex h-full min-h-0 w-full flex-col items-center gap-2 rounded-lg px-1 py-3 text-content-secondary hover:bg-surface-hover focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
          aria-expanded={false}
          aria-label={
            collapsedDone
              ? t('board:lane.expandCollapsed', { lane: labels.name[lane], count: entry.count })
              : t('board:lane.peek', { lane: labels.name[lane], count: entry.count })
          }
          onClick={() => (collapsedDone ? onCollapse(lane, false) : onPeek(lane, true))}
        >
          {cleared ? (
            <Check className="size-4 text-st-done" strokeWidth={2.25} aria-hidden="true" />
          ) : (
            <LaneGlyph lane={lane} className={cn('size-4', LANE_GLYPH_TONES[lane])} />
          )}
          <span className="font-mono text-xs font-semibold tabular-nums text-content">
            {entry.count}
          </span>
          <h2
            id={headingId}
            className={cn(
              'text-sm font-semibold tracking-[.08em] [writing-mode:vertical-rl]',
              cleared ? 'text-st-done' : 'text-content-secondary',
            )}
          >
            {cleared ? t('board:lane.cleared') : labels.name[lane]}
          </h2>
        </button>
      </section>
    )
  }

  const overflowLabel =
    lane === 'attention'
      ? t('board:lane.moreAttention', { count: hidden })
      : t('board:lane.more', { count: hidden })
  const revealHidden = () => {
    const scroller = scrollerRef.current
    if (!scroller) return
    if (virtual) {
      scroller.scrollBy({ top: scroller.clientHeight * 0.8, behavior: 'smooth' })
      return
    }
    const bottom = scroller.getBoundingClientRect().bottom
    const first = [...scroller.querySelectorAll<HTMLElement>('[data-board-card]')].find(
      (card) => card.getBoundingClientRect().bottom > bottom + 8,
    )
    first?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }

  const card = (item: BoardItem) => (
    <AgentCard
      key={item.id}
      item={item}
      now={now}
      phone={false}
      active={item.id === activeId}
      handlers={handlers}
    />
  )

  return (
    <section
      aria-labelledby={headingId}
      data-board-lane={lane}
      data-lane-mode="expanded"
      className={cn(
        'flex min-h-0 min-w-0 flex-col rounded-lg border border-stroke-soft bg-lane transition-opacity duration-(--d-fast)',
        dimmed && 'opacity-50',
      )}
    >
      <header className="flex h-11 flex-none items-center gap-2 px-3">
        <LaneGlyph
          lane={lane}
          spin={false}
          className={cn('size-4 flex-none', LANE_GLYPH_TONES[lane])}
        />
        <h2 id={headingId} className="flex-none text-sm font-semibold text-content">
          {labels.name[lane]}
        </h2>
        <span
          className={cn(
            'flex-none rounded-xs border px-1.5 font-mono text-xs leading-5 font-semibold tabular-nums',
            lane === 'attention'
              ? 'border-st-attn-border bg-st-attn-soft text-st-attn'
              : 'border-stroke-soft bg-surface-muted text-content-secondary',
          )}
        >
          {items.length}
        </span>
        <span className="min-w-0 flex-1" />
        {lane === 'done' ? (
          <>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="xs" className="text-content-muted">
                  {doneWindow === 'today' ? t('board:lane.windowToday') : t('board:lane.window7d')}
                  <ChevronDown aria-hidden="true" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-36">
                <DropdownMenuRadioGroup
                  value={doneWindow}
                  onValueChange={(value) => onDoneWindowChange(value as BoardDoneWindow)}
                >
                  <DropdownMenuRadioItem value="today">
                    {t('board:lane.windowToday')}
                  </DropdownMenuRadioItem>
                  <DropdownMenuRadioItem value="7d">
                    {t('board:lane.window7d')}
                  </DropdownMenuRadioItem>
                </DropdownMenuRadioGroup>
              </DropdownMenuContent>
            </DropdownMenu>
            <Button
              variant="ghost"
              size="icon-xs"
              className="text-content-muted"
              aria-label={t('board:lane.collapse', { lane: labels.name[lane] })}
              title={t('board:lane.collapse', { lane: labels.name[lane] })}
              onClick={() => onCollapse(lane, true)}
            >
              <PanelLeftClose aria-hidden="true" />
            </Button>
          </>
        ) : (
          <span className="min-w-0 truncate text-xs text-content-muted">{labels.order[lane]}</span>
        )}
        {/* 已完成泳道有自己的折叠键；其余空泳道临时展开后用这个键收回竖条。 */}
        {entry.count === 0 && lane !== 'done' && (
          <Button
            variant="ghost"
            size="icon-xs"
            className="text-content-muted"
            aria-label={t('board:lane.collapse', { lane: labels.name[lane] })}
            onClick={() => onPeek(lane, false)}
          >
            <PanelLeftClose aria-hidden="true" />
          </Button>
        )}
      </header>
      {lane === 'attention' && (
        <span
          aria-hidden="true"
          className="mx-3 mb-1 h-1 flex-none rounded-full bg-[image:var(--hatch-attn)]"
        />
      )}
      <div
        // 顶部留 4px：卡片 2px 焦点环外扩 2px，不被滚动区裁掉。
        ref={scrollerRef}
        data-lane-scroller={lane}
        className="relative min-h-0 flex-1 overflow-y-auto overscroll-contain px-2 pt-1 pb-2"
      >
        {shown.length === 0 ? (
          lane === 'done' ? (
            // 今天没有完成的任务时给出可操作的出路：切到 7 天，或去全部历史会话（下方）。
            <div className="flex flex-col items-center gap-2 px-1 py-6 text-center text-xs text-content-muted">
              <p>
                {doneWindow === 'today'
                  ? t('board:lane.doneEmptyToday')
                  : t('board:lane.doneEmptyWeek')}
              </p>
              {doneWindow === 'today' && offerWeek && (
                <Button variant="outline" size="xs" onClick={() => onDoneWindowChange('7d')}>
                  {t('board:lane.switchToWeek')}
                </Button>
              )}
            </div>
          ) : (
            <p className="px-1 py-6 text-center text-xs text-content-muted">
              {lane === 'attention' ? t('board:lane.attentionEmpty') : t('board:lane.empty')}
            </p>
          )
        ) : virtual ? (
          <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((row) => {
              const item = shown[row.index]
              if (!item) return null
              return (
                <div
                  key={row.key}
                  data-index={row.index}
                  ref={virtualizer.measureElement}
                  className="absolute inset-x-0 top-0"
                  style={{ transform: `translateY(${row.start}px)` }}
                >
                  {card(item)}
                </div>
              )
            })}
          </div>
        ) : (
          <div className="flex flex-col gap-2">{shown.map(card)}</div>
        )}
        {lane === 'done' && (
          <Button
            variant="ghost"
            size="sm"
            className="mt-2 w-full text-content-secondary"
            onClick={onOpenHistory}
          >
            {items.length > shown.length
              ? t('board:lane.historyMore', { count: items.length - shown.length })
              : t('board:lane.history')}
            <ArrowRight aria-hidden="true" />
          </Button>
        )}
      </div>
      {/* 溢出摘要吸附在泳道底部、滚动区之外：不盖住卡片上的按钮，滚到底后自动消失。 */}
      {hidden > 0 && (
        <div className="flex h-9 flex-none items-center justify-center border-t border-stroke-soft px-2">
          <button
            type="button"
            data-lane-overflow={lane}
            className={cn(
              'flex h-7 items-center gap-1 rounded-full border px-3 text-xs font-medium focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus',
              lane === 'attention'
                ? 'border-st-attn-border bg-st-attn-soft text-st-attn'
                : 'border-stroke bg-solid text-content-secondary',
            )}
            onClick={revealHidden}
          >
            {overflowLabel}
            <ArrowDown className="size-3.5" aria-hidden="true" />
          </button>
        </div>
      )}
    </section>
  )
}
