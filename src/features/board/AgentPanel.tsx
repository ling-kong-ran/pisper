// ⌘J 代理面板：从左侧滑出的 320px 覆盖层，按泳道分组列出全部代理（规格 4.2）。
// 这是 lanes 原设计里竖向代理栏的内容，代理多于 10 个时仍然好扫读；但它只是临时浮层，不是常驻侧栏。
// 每项是一张迷你卡片 + 一行细节 + 内联允许 / 拒绝（critical 只有“查看”）。Esc 关闭，焦点回到打开前的位置；
// ↑ / ↓ 在条目之间移动，↵ 打开会话并关闭面板。
import { useRef, type KeyboardEvent } from 'react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import { AgentMiniRow, type StripItemHandlers } from './AgentMiniCard'
import { useStripWords } from './strip-presentation'
import type { BoardItem } from './board-model'
import { LaneGlyph } from './LaneGlyph'
import { useLaneLabels } from './lane-labels'

const LANE_TONE: Record<BoardLane, string> = {
  attention: 'text-st-attn',
  running: 'text-st-run',
  queued: 'text-st-queue',
  done: 'text-st-done',
}

function titlebarBottom() {
  if (typeof document === 'undefined') return 0
  const bar = document.querySelector('[data-app-titlebar]')
  return bar ? Math.max(0, Math.round(bar.getBoundingClientRect().bottom)) : 0
}

export function AgentPanel({
  open,
  onOpenChange,
  lanes,
  queueAvailable,
  loading,
  currentId,
  now,
  handlers,
  returnFocus,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  lanes: Record<BoardLane, BoardItem[]>
  queueAvailable: boolean
  loading: boolean
  currentId: string
  now: number
  handlers: StripItemHandlers
  returnFocus: () => HTMLElement | null
}) {
  const { t } = useI18n()
  const labels = useLaneLabels()
  const word = useStripWords()
  const listRef = useRef<HTMLDivElement>(null)
  const shownLanes = (['attention', 'running', 'queued', 'done'] as const).filter(
    (lane) => lane !== 'queued' || queueAvailable,
  )
  const total = shownLanes.reduce((sum, lane) => sum + lanes[lane].length, 0)

  // ↑ / ↓ 只在条目的主按钮之间移动；行内的允许 / 拒绝仍走 Tab。
  const moveFocus = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
    const rows = [
      ...(listRef.current?.querySelectorAll<HTMLButtonElement>('[data-agent-row] > button') ?? []),
    ]
    if (!rows.length) return
    event.preventDefault()
    const index = rows.findIndex((row) => row === document.activeElement)
    const next =
      index < 0
        ? event.key === 'ArrowDown'
          ? 0
          : rows.length - 1
        : Math.max(0, Math.min(rows.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))
    rows[next]?.focus()
  }

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="left"
        showCloseButton={false}
        data-agent-panel
        className="gap-0 p-0"
        // 不盖住 48px 标题栏（舰队计量、窗口控件保持可见）；手机 App 平板没有标题栏时贴顶。
        style={{
          top: titlebarBottom(),
          height: 'auto',
          bottom: 'var(--pisper-safe-area-bottom)',
          width: 320,
          maxWidth: 'calc(100vw - 32px)',
        }}
        onOpenAutoFocus={(event) => {
          // 焦点落在当前会话（没有则第一项），而不是面板容器本身。
          const target =
            listRef.current?.querySelector<HTMLButtonElement>('[data-current] > button') ??
            listRef.current?.querySelector<HTMLButtonElement>('[data-agent-row] > button')
          if (!target) return
          event.preventDefault()
          target.focus()
        }}
        onCloseAutoFocus={(event) => {
          const target = returnFocus()
          if (!target?.isConnected) return
          event.preventDefault()
          target.focus()
        }}
      >
        <SheetHeader className="gap-0.5 border-b border-stroke-soft px-4 pt-3.5 pb-3">
          <SheetTitle className="text-sm font-semibold">{t('board:agents.title')}</SheetTitle>
          <SheetDescription className="text-xs">{t('board:agents.description')}</SheetDescription>
        </SheetHeader>
        <div
          ref={listRef}
          className="min-h-0 flex-1 overflow-y-auto px-2 py-2"
          onKeyDown={moveFocus}
        >
          {loading && total === 0 ? (
            <PageState kind="loading" title={t('board:page.loading')} />
          ) : total === 0 ? (
            <PageState kind="empty" title={t('board:agents.empty')} />
          ) : (
            shownLanes.map((lane) =>
              lanes[lane].length ? (
                <section
                  key={lane}
                  aria-label={t('board:agents.lane', {
                    lane: labels.name[lane],
                    count: lanes[lane].length,
                  })}
                  data-agent-lane={lane}
                  className="mb-2 grid gap-0.5"
                >
                  <h3
                    className={cn(
                      'flex items-center gap-1.5 px-2 pt-1 pb-0.5 text-xs font-semibold',
                      LANE_TONE[lane],
                    )}
                  >
                    <LaneGlyph lane={lane} spin={false} className="size-3.5" />
                    {labels.name[lane]}
                    <span className="font-mono tabular-nums">{lanes[lane].length}</span>
                  </h3>
                  {lane === 'attention' && (
                    <span
                      aria-hidden="true"
                      className="mx-2 mb-1 block h-1 rounded-full bg-[image:var(--hatch-attn)]"
                    />
                  )}
                  {lanes[lane].map((item) => (
                    <AgentMiniRow
                      key={item.id}
                      item={item}
                      now={now}
                      current={item.id === currentId}
                      handlers={handlers}
                      onOpen={handlers.onOpen}
                      word={word(item)}
                    />
                  ))}
                </section>
              ) : null,
            )
          )}
        </div>
      </SheetContent>
    </Sheet>
  )
}
