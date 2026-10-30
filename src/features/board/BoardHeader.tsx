// 看板页头（52px）：标题与副标题、注意力胶囊、项目筛选、状态筛选、“泳道 / 项目”分组切换与 ⋯ 菜单。
// 胶囊用 --st-attn-soft 浅底 + 琥珀描边，只有左侧小图标块是实心琥珀，避免和泳道连成一片橙色。
// 宽度不足 1280 时项目筛选收成下拉，分组切换只显示图标（3.11）。
// 新建项目、管理项目放在“项目 ▾”下拉里：项目筛选是下拉时并入其中，是分段控件（或还没有卡片）时
// 另放一个“项目 ▾”按钮。
// 触屏：筛选、分组与 ⋯ 都是 44px（按钮与分段控件的触屏尺寸）；胶囊里的“逐个处理”保持 24px 可见高度，
// 命中区用伪元素补到 44px（页头 52px，伪元素不出页头）。
import {
  Check,
  ChevronDown,
  Ellipsis,
  Folder,
  FolderCog,
  FolderPlus,
  ListFilter,
  PanelTopClose,
  PanelTopOpen,
  TriangleAlert,
} from 'lucide-react'
import { BOARD_LANES, type BoardLane } from '@shared/session-board-status.mjs'
import { useI18n } from '@/app/use-i18n'
import { LaneBarsIcon } from '@/components/app/LaneBarsIcon'
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
import { Kbd } from '@/components/ui/kbd'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { TOUCH_HIT_AREA } from '@/lib/touch-target'
import { cn } from '@/lib/utils'
import type { BoardProject } from './board-model'
import type { BoardGrouping } from './board-view-store'
import { projectDisambiguation } from './card-presentation'
import { BOARD_FILTER_ID } from './new-task'
import { useLaneLabels } from './lane-labels'

export type BoardHeaderProject = BoardProject & { label: string }

type BoardHeaderProps = {
  total: number
  projectCount: number
  running: number
  // 需要你的计数按全部事项算（不受筛选影响）；hidden 是被当前筛选藏起来的条数。
  attention: {
    total: number
    approvals: number
    questions: number
    failures: number
    hidden: number
  }
  // 数据已就绪：只有这时才能下“没有需要你处理的事项”的结论（加载中不能报平安）。
  ready: boolean
  projects: BoardHeaderProject[]
  projectFilter: readonly string[]
  laneFilter: readonly BoardLane[]
  availableLanes: readonly BoardLane[]
  groupBy: BoardGrouping
  narrow: boolean
  deckCollapsed: boolean
  triageKey: string
  onTriage: () => void
  onProjectFilter: (projects: string[]) => void
  onToggleProject: (project: string) => void
  onToggleLane: (lane: BoardLane) => void
  onClearLanes: () => void
  onGroupBy: (groupBy: BoardGrouping) => void
  onDeckCollapsed: (collapsed: boolean) => void
  // 正在打开系统目录对话框：“新建项目…”暂不可用，避免叠加窗口。
  projectPicking: boolean
  onNewProject: () => void
  onManageProjects: () => void
}

// 分段控件里最多放 3 个项目；更多时（或窄宽度）改用下拉，避免把右侧挤出页头。
const SEGMENT_PROJECT_LIMIT = 3

export function BoardHeader({
  total,
  projectCount,
  running,
  attention,
  ready,
  projects,
  projectFilter,
  laneFilter,
  availableLanes,
  groupBy,
  narrow,
  deckCollapsed,
  triageKey,
  onTriage,
  onProjectFilter,
  onToggleProject,
  onToggleLane,
  onClearLanes,
  onGroupBy,
  onDeckCollapsed,
  projectPicking,
  onNewProject,
  onManageProjects,
}: BoardHeaderProps) {
  const { t } = useI18n()
  const laneLabels = useLaneLabels()
  const segmented = !narrow && projects.length > 0 && projects.length <= SEGMENT_PROJECT_LIMIT
  // 同名项目附上简写路径，否则筛选里会出现两个一模一样的“pisper”。
  const hints = projectDisambiguation(projects)
  const singleProject = projectFilter.length === 1 ? projectFilter[0] : ''
  const projectSummary =
    projectFilter.length === 0
      ? t('board:filter.allProjects')
      : projectFilter.length === 1
        ? projects.find((project) => project.key === projectFilter[0])?.label || projectFilter[0]
        : t('board:filter.projectsSelected', { count: projectFilter.length })
  const laneSummary =
    laneFilter.length === 0
      ? t('board:filter.allStatuses')
      : laneFilter.map((lane) => laneLabels.name[lane]).join(' · ')
  const projectItems = (
    <>
      <DropdownMenuItem disabled={projectPicking} onSelect={onNewProject}>
        <FolderPlus aria-hidden="true" />
        {t('board:project.new')}
      </DropdownMenuItem>
      <DropdownMenuItem onSelect={onManageProjects}>
        <FolderCog aria-hidden="true" />
        {t('board:project.manage')}
      </DropdownMenuItem>
    </>
  )
  // 项目筛选不是下拉（分段控件、没有项目或还没有卡片）时，单独给一个“项目 ▾”。
  const projectMenu = (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          className="flex-none"
          aria-label={t('board:project.menu')}
          title={t('board:project.menu')}
        >
          <FolderCog aria-hidden="true" />
          <span className={cn(narrow && 'max-[1179px]:sr-only')}>{t('board:filter.projects')}</span>
          <ChevronDown aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        {projectItems}
      </DropdownMenuContent>
    </DropdownMenu>
  )
  const breakdown = [
    attention.approvals ? t('board:attention.approvals', { count: attention.approvals }) : '',
    attention.questions ? t('board:attention.questions', { count: attention.questions }) : '',
    attention.failures ? t('board:attention.failures', { count: attention.failures }) : '',
  ].filter(Boolean)

  return (
    <header className="flex h-[52px] flex-none items-center gap-3 px-4">
      <div className="flex min-w-0 flex-none items-baseline gap-3">
        <h1 className="text-xl leading-7 font-[650] tracking-[-.015em] text-content">
          {t('board:page.title')}
        </h1>
        {total > 0 && (
          <p className="truncate text-xs text-content-muted max-[1279px]:hidden">
            {t('board:page.summary', { agents: total, projects: projectCount, running })}
          </p>
        )}
      </div>
      {attention.total > 0 ? (
        <div
          role="group"
          aria-label={t('board:attention.label', { count: attention.total })}
          data-attention-capsule
          className="flex h-8 min-w-0 items-center gap-2 rounded-sm border border-st-attn-border bg-st-attn-soft pr-1 pl-1 text-xs"
        >
          <span
            aria-hidden="true"
            className="grid size-6 flex-none place-items-center rounded-xs bg-st-attn-solid text-on-attn ring-1 ring-st-attn-ring ring-inset"
          >
            <TriangleAlert className="size-3.5" strokeWidth={2.25} />
          </span>
          <b className="flex-none font-semibold text-st-attn">
            {t('board:attention.count', { count: attention.total })}
          </b>
          {breakdown.length > 0 && (
            <span className="min-w-0 truncate text-content-secondary max-[1179px]:hidden">
              {breakdown.join(' · ')}
            </span>
          )}
          {attention.hidden > 0 && (
            <span className="min-w-0 flex-none text-content-muted">
              {t('board:attention.hidden', { count: attention.hidden })}
            </span>
          )}
          <Button
            variant="outline"
            size="xs"
            className={cn('flex-none bg-solid', TOUCH_HIT_AREA)}
            aria-keyshortcuts={triageKey || undefined}
            onClick={onTriage}
          >
            {t('board:attention.triage')}
            {triageKey && <Kbd className="ml-0.5">{triageKey}</Kbd>}
          </Button>
        </div>
      ) : ready ? (
        <p
          data-attention-capsule
          className="flex h-8 min-w-0 items-center gap-1.5 rounded-sm border border-stroke-soft bg-surface-subtle px-2.5 text-xs text-content-secondary"
        >
          <Check
            className="size-3.5 flex-none text-st-done"
            strokeWidth={2.25}
            aria-hidden="true"
          />
          <span className="truncate">{t('board:attention.clear')}</span>
        </p>
      ) : null}
      <span className="min-w-0 flex-1" />
      {/* 空看板没有可筛选或分组的卡片，右侧只留“项目 ▾”（新建、管理项目）。 */}
      {total === 0 && ready && projectMenu}
      {total > 0 && (
        <>
          {projects.length === 0 && projectMenu}
          {projects.length > 0 &&
            (segmented ? (
              <SegmentedControl
                id={BOARD_FILTER_ID}
                size="sm"
                aria-label={t('board:filter.projects')}
                value={projectFilter.length === 0 ? '' : singleProject || '\u0001'}
                onValueChange={(value) => onProjectFilter(value ? [value] : [])}
                className="flex-none"
                options={[
                  { value: '', label: t('board:filter.all'), count: total },
                  ...projects.map((project) => ({
                    value: project.key,
                    label: (
                      <span className="flex min-w-0 items-baseline gap-1 font-mono">
                        <span className="max-w-[14ch] truncate">{project.label}</span>
                        {hints.get(project.key) && (
                          <span className="max-w-[12ch] truncate text-content-muted">
                            {hints.get(project.key)}
                          </span>
                        )}
                      </span>
                    ),
                    count: project.count,
                  })),
                ]}
              />
            ) : (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    id={BOARD_FILTER_ID}
                    variant="outline"
                    size="sm"
                    className="max-w-[22ch] min-w-0 flex-none"
                    aria-label={t('board:filter.projectsTitle', { projects: projectSummary })}
                  >
                    <Folder aria-hidden="true" />
                    <span className="truncate">
                      {t('board:filter.projectsTitle', { projects: projectSummary })}
                    </span>
                    <ChevronDown aria-hidden="true" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="end"
                  // 登记的空项目会让列表变长：项目列表单独滚动，“新建项目… / 管理项目…”固定在底部，
                  // 不会被 320px 上限截成半行。
                  className="flex max-h-[min(30rem,var(--radix-dropdown-menu-content-available-height))] w-64 flex-col overflow-y-hidden"
                >
                  <DropdownMenuLabel>{t('board:filter.projects')}</DropdownMenuLabel>
                  <DropdownMenuCheckboxItem
                    checked={projectFilter.length === 0}
                    onCheckedChange={() => onProjectFilter([])}
                  >
                    {t('board:filter.all')}
                    <span className="ml-auto font-mono text-xs text-content-muted tabular-nums">
                      {total}
                    </span>
                  </DropdownMenuCheckboxItem>
                  <DropdownMenuSeparator />
                  <div
                    className="-mx-1 min-h-0 flex-1 overflow-y-auto px-1"
                    data-project-filter-list
                  >
                    {projects.map((project) => (
                      <DropdownMenuCheckboxItem
                        key={project.key}
                        checked={projectFilter.includes(project.key)}
                        onCheckedChange={() => onToggleProject(project.key)}
                        title={project.cwd}
                      >
                        <span className="flex min-w-0 flex-col">
                          <span className="truncate font-mono text-xs">{project.label}</span>
                          {hints.get(project.key) && (
                            <span className="truncate font-mono text-xs text-content-muted">
                              {hints.get(project.key)}
                            </span>
                          )}
                        </span>
                        <span className="ml-auto font-mono text-xs text-content-muted tabular-nums">
                          {project.count}
                        </span>
                      </DropdownMenuCheckboxItem>
                    ))}
                  </div>
                  <DropdownMenuSeparator />
                  {projectItems}
                </DropdownMenuContent>
              </DropdownMenu>
            ))}
          {projects.length > 0 && segmented && projectMenu}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                id={projects.length === 0 ? BOARD_FILTER_ID : undefined}
                variant="outline"
                size="sm"
                className="max-w-[24ch] min-w-0 flex-none"
                aria-label={t('board:filter.statusTitle', { statuses: laneSummary })}
              >
                <ListFilter aria-hidden="true" />
                <span className={cn('truncate', narrow && 'max-[1179px]:sr-only')}>
                  {t('board:filter.statusTitle', { statuses: laneSummary })}
                </span>
                <ChevronDown aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-52">
              <DropdownMenuLabel>{t('board:filter.statuses')}</DropdownMenuLabel>
              <DropdownMenuCheckboxItem
                checked={laneFilter.length === 0}
                onCheckedChange={onClearLanes}
              >
                {t('board:filter.all')}
              </DropdownMenuCheckboxItem>
              <DropdownMenuSeparator />
              {BOARD_LANES.filter((lane) => availableLanes.includes(lane)).map((lane) => (
                <DropdownMenuCheckboxItem
                  key={lane}
                  checked={laneFilter.includes(lane)}
                  onCheckedChange={() => onToggleLane(lane)}
                >
                  {laneLabels.name[lane]}
                </DropdownMenuCheckboxItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
          <SegmentedControl
            size="sm"
            aria-label={t('board:group.label')}
            value={groupBy}
            onValueChange={onGroupBy}
            className="flex-none"
            options={[
              {
                value: 'lane',
                label: narrow ? null : t('board:group.lane'),
                icon: <LaneBarsIcon className="size-3.5" aria-hidden="true" />,
                ariaLabel: t('board:group.lane'),
              },
              {
                value: 'project',
                label: narrow ? null : t('board:group.project'),
                icon: <Folder className="size-3.5" aria-hidden="true" />,
                ariaLabel: t('board:group.project'),
              },
            ]}
          />
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                className="flex-none"
                aria-label={t('board:page.moreActions')}
              >
                <Ellipsis aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <DropdownMenuItem onSelect={() => onDeckCollapsed(!deckCollapsed)}>
                {deckCollapsed ? (
                  <PanelTopOpen aria-hidden="true" />
                ) : (
                  <PanelTopClose aria-hidden="true" />
                )}
                {deckCollapsed ? t('board:deck.expand') : t('board:deck.collapse')}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </>
      )}
    </header>
  )
}
