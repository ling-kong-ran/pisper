// 分组 · 项目：列是四种状态（与泳道对齐），行是项目横带。带头有项目字形、名称、简写路径
// 与分段迷你状态条（每个代理一段，按状态着色，排队画成虚线）；横带可以折叠。
// 项目顺序与显示名沿用 workspace-order-store，与侧栏、启动台一致。
// 带头右侧的“⋯”菜单：在此项目新建任务（聚焦启动台并选中该项目）、重命名、恢复目录名、删除项目。
import { useMemo, useRef, useState } from 'react'
import { ChevronDown, Ellipsis, FolderPen, Plus, RotateCcw, Trash2 } from 'lucide-react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import type { StatusTone } from '@/components/app/status-glyph-kinds'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { AgentCard, type AgentCardHandlers } from './AgentCard'
import type { BoardItem } from './board-model'
import { boardCardTone, shortPath } from './card-presentation'
import { LaneGlyph } from './LaneGlyph'
import { useLaneLabels } from './lane-labels'

export type ProjectRow = {
  key: string
  cwd: string
  label: string
  // 显示名是用户自定义的（可以恢复目录名）。
  custom: boolean
  items: BoardItem[]
}

export type ProjectRowActions = {
  onNewTask: (row: ProjectRow) => void
  onRename: (row: ProjectRow) => void
  onResetName: (row: ProjectRow) => void
  onDelete: (row: ProjectRow) => void
  // 正在删除的项目键；删除进行中时所有“删除项目…”都不可用，避免并发删除。
  deletingKey: string
}

// 迷你状态条每个代理一段，按卡片状态着色（失败红、等待批准琥珀……），排队画成虚线。
const SEGMENT_CLASSES: Record<StatusTone, string> = {
  attn: 'bg-st-attn-fill',
  fail: 'bg-st-fail-fill',
  run: 'bg-st-run-fill',
  queue: 'border border-dashed border-st-queue-fill bg-transparent',
  done: 'bg-st-done-fill',
}

const LANE_TONES: Record<BoardLane, string> = {
  attention: 'text-st-attn',
  running: 'text-st-run',
  queued: 'text-st-queue',
  done: 'text-st-done',
}

type ProjectMatrixProps = {
  rows: ProjectRow[]
  lanes: readonly BoardLane[]
  template: string
  now: number
  activeId: string
  dimLane: (lane: BoardLane) => boolean
  handlers: AgentCardHandlers
  actions: ProjectRowActions
}

function ProjectBandMenu({ row, actions }: { row: ProjectRow; actions: ProjectRowActions }) {
  const { t } = useI18n()
  // “在此项目新建任务”要把焦点交给启动台：等菜单关闭、焦点归还阶段再交，否则会被菜单拉回触发键。
  const newTaskRef = useRef(false)
  const hasFolder = Boolean(row.cwd)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className="mr-1.5 flex-none"
          aria-label={t('board:project.actionsFor', { project: row.label })}
          title={t('board:project.actionsFor', { project: row.label })}
        >
          <Ellipsis aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        className="w-52"
        onCloseAutoFocus={(event) => {
          if (!newTaskRef.current) return
          newTaskRef.current = false
          event.preventDefault()
          actions.onNewTask(row)
        }}
      >
        {hasFolder && (
          <>
            <DropdownMenuItem onSelect={() => (newTaskRef.current = true)}>
              <Plus aria-hidden="true" />
              {t('board:project.newTaskHere')}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => actions.onRename(row)}>
              <FolderPen aria-hidden="true" />
              {t('board:project.rename')}
            </DropdownMenuItem>
            {row.custom && (
              <DropdownMenuItem onSelect={() => actions.onResetName(row)}>
                <RotateCcw aria-hidden="true" />
                {t('board:project.resetName')}
              </DropdownMenuItem>
            )}
            <DropdownMenuSeparator />
          </>
        )}
        <DropdownMenuItem
          variant="destructive"
          disabled={Boolean(actions.deletingKey)}
          onSelect={() => actions.onDelete(row)}
        >
          <Trash2 aria-hidden="true" />
          {t('board:project.delete')}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

export function ProjectMatrix({
  rows,
  lanes,
  template,
  now,
  activeId,
  dimLane,
  handlers,
  actions,
}: ProjectMatrixProps) {
  const { t } = useI18n()
  const labels = useLaneLabels()
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set())
  const counts = useMemo(() => {
    const result = Object.fromEntries(lanes.map((lane) => [lane, 0])) as Record<BoardLane, number>
    for (const row of rows)
      for (const item of row.items) if (item.lane in result) result[item.lane] += 1
    return result
  }, [lanes, rows])
  const toggle = (key: string) =>
    setCollapsed((current) => {
      const next = new Set(current)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })

  return (
    <div
      className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto overscroll-contain px-4 pb-4"
      data-board-matrix
    >
      <div
        aria-hidden="true"
        className="sticky top-0 z-10 grid gap-3 bg-background pb-1"
        style={{ gridTemplateColumns: template }}
      >
        {lanes.map((lane) => (
          <div key={lane} className="flex h-8 min-w-0 items-center gap-2 px-2">
            <LaneGlyph lane={lane} className={cn('size-4 flex-none', LANE_TONES[lane])} />
            <span className="truncate text-sm font-semibold text-content">{labels.name[lane]}</span>
            <span className="font-mono text-xs text-content-muted tabular-nums">
              {counts[lane]}
            </span>
          </div>
        ))}
      </div>
      {rows.map((row) => {
        const open = !collapsed.has(row.key)
        const bodyId = `board-project-${row.key.replace(/[^a-z0-9]/gi, '-')}`
        return (
          <section
            key={row.key}
            aria-label={row.label}
            className="flex-none rounded-lg border border-stroke-soft bg-lane"
          >
            <div className="flex min-w-0 items-center rounded-lg hover:bg-surface-hover">
              <button
                type="button"
                aria-expanded={open}
                aria-controls={bodyId}
                onClick={() => toggle(row.key)}
                className="flex h-11 min-w-0 flex-1 items-center gap-2.5 rounded-lg px-3 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
              >
                <span
                  aria-hidden="true"
                  className="grid size-6 flex-none place-items-center rounded-sm border border-stroke-strong bg-solid font-mono text-xs font-semibold text-content uppercase"
                >
                  {Array.from(row.label)[0] || '·'}
                </span>
                <span className="min-w-0 truncate font-mono text-sm font-semibold text-content">
                  {row.label}
                </span>
                <span
                  className="min-w-0 truncate font-mono text-xs text-content-muted max-[1279px]:hidden"
                  title={row.cwd}
                >
                  {shortPath(row.cwd)}
                </span>
                <span
                  role="img"
                  aria-label={t('board:project.miniBar', { count: row.items.length })}
                  className="ml-auto flex h-2 w-40 max-w-[30%] flex-none gap-0.5"
                >
                  {row.items.map((item) => (
                    <span
                      key={item.id}
                      className={cn(
                        'h-full min-w-1 flex-1 rounded-[2px]',
                        SEGMENT_CLASSES[boardCardTone(item)],
                      )}
                    />
                  ))}
                </span>
                <span className="font-mono text-xs text-content-muted tabular-nums">
                  {row.items.length}
                </span>
                <ChevronDown
                  className={cn(
                    'size-4 flex-none text-content-muted transition-transform duration-(--d-fast)',
                    !open && '-rotate-90',
                  )}
                  aria-hidden="true"
                />
              </button>
              <ProjectBandMenu row={row} actions={actions} />
            </div>
            {open && (
              <div
                id={bodyId}
                className="grid gap-3 px-2 pb-2"
                style={{ gridTemplateColumns: template }}
              >
                {lanes.map((lane) => {
                  const cell = row.items.filter((item) => item.lane === lane)
                  return (
                    <div
                      key={lane}
                      data-board-lane={lane}
                      className={cn(
                        'flex min-w-0 flex-col gap-2 transition-opacity duration-(--d-fast)',
                        dimLane(lane) && 'opacity-50',
                      )}
                    >
                      {cell.length === 0 ? (
                        <span className="grid h-10 place-items-center rounded-card border border-dashed border-stroke-soft text-xs text-content-muted">
                          {t('board:lane.empty')}
                        </span>
                      ) : (
                        cell.map((item) => (
                          <AgentCard
                            key={item.id}
                            item={item}
                            now={now}
                            phone={false}
                            active={item.id === activeId}
                            handlers={handlers}
                          />
                        ))
                      )}
                    </div>
                  )
                })}
              </div>
            )}
          </section>
        )
      })}
    </div>
  )
}
