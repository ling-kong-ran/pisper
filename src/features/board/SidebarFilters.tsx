// 全部代理页的侧栏筛选（规格 v2 第 2.3 节）：在这一页，侧栏的会话分组收成两组筛选——
// 状态（全部 · 需要你 · 运行中 · 排队 · 今天完成，各带计数）与项目（每个项目一行，带计数；
// 分组标题的 ⋯ 里是“添加项目 / 管理项目”）。代理只在主区列一次，侧栏不再重复列出会话行。
// 筛选状态就是 /board 的 pisper-board-view（useBoardViewStore）：与主区、窄轨计数、页内“清除筛选”同一份，
// 不新增偏好键。计数与主区同一份“出现在全部代理上的代理”（overview-model），按筛选前计。
// 点一行只看这一项，再点一次回到全部；“全部”清掉状态与项目两种筛选，也只在两者都没筛选时按下
// （只筛了项目时“全部 7”若仍按下，读起来像主区在显示全部 7 个）。按钮带 aria-pressed，↑ / ↓ 在筛选之间移动。
// 本模块随看板 store 懒加载（入口预算），和会话分组同一个块。
import {
  lazy,
  Suspense,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react'
import { Ellipsis, Folder, FolderCog, FolderPlus, History, LayoutList } from 'lucide-react'
import { useHref } from 'react-router-dom'
import type { BoardLane } from '@shared/session-board-status.mjs'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import type { StatusGlyphKind } from '@/components/app/status-glyph-kinds'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { cn } from '@/lib/utils'
import { boardProjects } from './board-model'
import { useBoardViewStore } from './board-view-store'
import { BOARD_FILTER_ID } from './new-task'
import { overviewFilterCounts } from './overview-model'
import { SidebarSkeleton } from './SidebarSessions'
import { useBoardItems } from './useBoardItems'
import { useLabeledProjects } from './useLabeledProjects'
import { useMinute } from './useNow'
import { useProjectCommands } from './useProjectCommands'

const ManageProjectsDialog = lazy(() =>
  import('./ManageProjectsDialog').then((module) => ({ default: module.ManageProjectsDialog })),
)
const WorkspacePicker = lazy(() =>
  import('@/components/WorkspacePicker').then((module) => ({ default: module.WorkspacePicker })),
)

export type SidebarBoardFiltersProps = {
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  // 添加项目选好目录（已记成派发目标）之后打开派发对话框，与全部代理页的“在此项目新建任务”一致。
  onNewTask: () => void
  onOpenHistory: () => void
}

const STATUS_ROWS: ReadonlyArray<{ lane: BoardLane; glyph: StatusGlyphKind }> = [
  { lane: 'attention', glyph: 'attention' },
  { lane: 'running', glyph: 'run' },
  { lane: 'queued', glyph: 'queued' },
  { lane: 'done', glyph: 'done' },
]

const ROW =
  'flex h-8 w-full min-w-0 items-center gap-2.5 rounded-[var(--r-md)] px-2.5 text-left text-[13.5px] text-content transition-colors duration-(--d-fast) hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11'
const HEADING = 'flex h-7 min-w-0 flex-1 items-center px-2.5 text-xs font-medium text-content-muted'

function filterButtons(root: HTMLElement | null) {
  return root ? [...root.querySelectorAll<HTMLElement>('[data-sidebar-filter]')] : []
}

export function SidebarBoardFilters({
  notify,
  requestConfirm,
  requestText,
  onNewTask,
  onOpenHistory,
}: SidebarBoardFiltersProps) {
  const { t } = useI18n()
  const { items, capabilities, isLoading } = useBoardItems('today')
  const minute = useMinute()
  const lanes = useBoardViewStore((state) => state.lanes)
  const projectFilter = useBoardViewStore((state) => state.projects)
  const setLaneFilter = useBoardViewStore((state) => state.setLaneFilter)
  const setProjectFilter = useBoardViewStore((state) => state.setProjectFilter)
  const clearFilters = useBoardViewStore((state) => state.clearFilters)
  const counts = useMemo(
    () => overviewFilterCounts(items, { now: minute, capabilities }),
    [capabilities, items, minute],
  )
  const projects = useLabeledProjects(useMemo(() => boardProjects(items), [items]))
  const rootRef = useRef<HTMLDivElement>(null)
  const root = useHref('/')

  const commands = useProjectCommands({
    notify,
    requestConfirm,
    requestText,
    onProjectChosen: onNewTask,
    announceChoice: true,
  })
  const [manageMounted, setManageMounted] = useState(false)
  const [manageOpen, setManageOpen] = useState(false)
  // 菜单项会打开对话框：等菜单关闭、焦点归还阶段再打开，否则焦点会被菜单拉回触发键。
  const afterClose = useRef<(() => void) | null>(null)
  const { mounted: pickerMounted, ...pickerProps } = commands.picker

  const laneName: Record<BoardLane, string> = {
    attention: t('board:sidebar.group.attention'),
    running: t('board:sidebar.group.running'),
    queued: t('board:sidebar.group.queued'),
    done: t('board:sidebar.group.done'),
  }
  // 单选：点一项只看它，再点一次回到全部（存下的旧筛选可能是多选，照样按 includes 标出按下）。
  const selectLane = (lane: BoardLane) =>
    setLaneFilter(lanes.length === 1 && lanes[0] === lane ? [] : [lane])
  const selectProject = (key: string) =>
    setProjectFilter(projectFilter.length === 1 && projectFilter[0] === key ? [] : [key])

  // ↑ / ↓ / Home / End 在全部筛选按钮之间移动（状态与项目连成一列，与会话行的方向键一致）。
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) return
    const target = event.target as HTMLElement
    if (!target.hasAttribute('data-sidebar-filter')) return
    const buttons = filterButtons(rootRef.current)
    const index = buttons.indexOf(target)
    const next =
      event.key === 'ArrowDown'
        ? buttons[index + 1]
        : event.key === 'ArrowUp'
          ? buttons[index - 1]
          : event.key === 'Home'
            ? buttons[0]
            : event.key === 'End'
              ? buttons.at(-1)
              : undefined
    if (!next) return
    event.preventDefault()
    next.focus()
  }

  if (isLoading) return <SidebarSkeleton label={t('board:sidebar.loading')} />

  const statusRows = STATUS_ROWS.filter(
    // 排队只在 Runtime 声明 sessionQueue 且确有排队时出现（规格 2.1、A9），不做假数据。
    ({ lane }) =>
      lane !== 'queued' || (capabilities.sessionQueue === true && counts.lanes.queued > 0),
  )
  const projectsLabel = t('board:filter.projects')
  return (
    <div
      ref={rootRef}
      data-sidebar-filters
      className="flex flex-col gap-3 pb-2"
      onKeyDown={onKeyDown}
    >
      <section aria-labelledby="sidebar-filter-status">
        <h2 id="sidebar-filter-status" className={HEADING}>
          {t('board:filter.statuses')}
        </h2>
        <ul className="m-0 flex list-none flex-col gap-px p-0">
          <li>
            <FilterButton
              id={BOARD_FILTER_ID}
              filter="all"
              pressed={lanes.length === 0 && projectFilter.length === 0}
              label={t('board:sidebar.filter.all')}
              count={counts.total}
              icon={
                <LayoutList
                  className="size-3.5 text-content-muted"
                  strokeWidth={1.75}
                  aria-hidden="true"
                />
              }
              onClick={clearFilters}
            />
          </li>
          {statusRows.map(({ lane, glyph }) => {
            const count = counts.lanes[lane]
            const attention = lane === 'attention' && count > 0
            return (
              <li key={lane}>
                <FilterButton
                  filter={lane}
                  pressed={lanes.includes(lane)}
                  label={laneName[lane]}
                  count={count}
                  attention={attention}
                  icon={
                    <StatusGlyph
                      status={glyph}
                      spin={false}
                      className={cn(
                        'size-4',
                        count === 0 && lane === 'attention' && 'text-content-faint',
                      )}
                    />
                  }
                  onClick={() => selectLane(lane)}
                />
              </li>
            )
          })}
        </ul>
      </section>
      <section aria-labelledby="sidebar-filter-projects">
        <div className="flex items-center pr-1">
          <h2 id="sidebar-filter-projects" className={HEADING}>
            {projectsLabel}
          </h2>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                data-sidebar-projects-menu
                aria-label={t('board:sidebar.filter.projectActions')}
                title={t('board:sidebar.filter.projectActions')}
                className="grid size-6 flex-none place-items-center rounded-[var(--r-sm)] text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-focus pointer-coarse:size-11"
              >
                <Ellipsis className="size-3.5" aria-hidden="true" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              className="w-48"
              onCloseAutoFocus={(event) => {
                const next = afterClose.current
                if (!next) return
                afterClose.current = null
                event.preventDefault()
                next()
              }}
            >
              <DropdownMenuItem
                disabled={commands.picking}
                onSelect={() => (afterClose.current = () => void commands.newProject())}
              >
                <FolderPlus aria-hidden="true" />
                {t('board:sidebar.filter.addProject')}
              </DropdownMenuItem>
              <DropdownMenuItem
                onSelect={() =>
                  (afterClose.current = () => {
                    setManageMounted(true)
                    setManageOpen(true)
                  })
                }
              >
                <FolderCog aria-hidden="true" />
                {t('board:project.manage')}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        {projects.length ? (
          <ul className="m-0 flex list-none flex-col gap-px p-0">
            {projects.map((project) => (
              <li key={project.key}>
                <FilterButton
                  filter={`project:${project.key}`}
                  pressed={projectFilter.includes(project.key)}
                  label={project.label}
                  mono={Boolean(project.cwd) && !project.custom}
                  count={counts.projects[project.key] ?? 0}
                  icon={
                    <Folder
                      className="size-3.5 text-content-muted"
                      strokeWidth={1.75}
                      aria-hidden="true"
                    />
                  }
                  onClick={() => selectProject(project.key)}
                />
              </li>
            ))}
          </ul>
        ) : (
          <p className="px-2.5 text-xs leading-7 text-content-faint">
            {t('board:sidebar.filter.noProjects')}
          </p>
        )}
      </section>
      <a
        href={root.replace(/\/$/, '') + '/chat/history'}
        className="flex h-8 items-center gap-2 rounded-[var(--r-md)] px-2.5 text-[13px] text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11"
        onClick={(event) => {
          if (
            event.button === 0 &&
            !event.metaKey &&
            !event.ctrlKey &&
            !event.shiftKey &&
            !event.altKey
          ) {
            event.preventDefault()
            onOpenHistory()
          }
        }}
      >
        <History className="size-3.5" strokeWidth={1.75} aria-hidden="true" />
        {t('board:sidebar.history')}
      </a>
      {pickerMounted && (
        <Suspense fallback={null}>
          <WorkspacePicker {...pickerProps} />
        </Suspense>
      )}
      {manageMounted && (
        <Suspense fallback={null}>
          <ManageProjectsDialog
            open={manageOpen}
            onOpenChange={setManageOpen}
            commands={commands}
          />
        </Suspense>
      )}
    </div>
  )
}

function FilterButton({
  id,
  filter,
  pressed,
  label,
  count,
  icon,
  attention = false,
  mono = false,
  onClick,
}: {
  id?: string
  filter: string
  pressed: boolean
  label: string
  count: number
  icon: ReactNode
  attention?: boolean
  mono?: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      id={id}
      data-sidebar-filter={filter}
      aria-pressed={pressed}
      title={label}
      className={cn(
        ROW,
        pressed && 'bg-surface-active font-medium hover:bg-surface-active',
        // 琥珀只表示需要你（规格 6.1）：只有需要你且确有事项时整行染琥珀文字。
        attention && 'text-warning-strong',
      )}
      onClick={onClick}
    >
      <span className="grid size-4 flex-none place-items-center">{icon}</span>
      <span className={cn('min-w-0 flex-1 truncate', mono && 'font-mono text-[12.5px]')}>
        {label}
      </span>
      <span
        data-sidebar-filter-count
        className={cn(
          'flex-none text-xs tabular-nums',
          attention ? 'font-semibold' : 'text-content-muted',
        )}
      >
        {count}
      </span>
    </button>
  )
}
