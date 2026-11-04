// 全部代理页（桌面）的页内筛选：标题下一行，左边状态分段（全部 · 需要你 · 运行中 · 排队 · 今天完成，各带计数），
// 右边项目下拉与项目的 ⋯。侧栏在每个页面都一样，不再兼做这页的筛选器（同一块侧栏换个页面就换了用法，容易误会）。
// 筛选状态仍是 /board 的 pisper-board-view（useBoardViewStore）：与窄轨计数点进来的筛选同一份。
// 状态是单选；存下的旧多选筛选（手机看板）在这里按“全部”以外的第一项显示。
// 项目与“管理项目”同一来源（useCatalogProjects）：前几天的项目、还没有会话的登记项目都在下拉里。
// ⋯ 里始终有添加项目 / 管理项目；选中某个项目时，它自己的操作（新建任务、改名、删除）直接在这里，
// 不必先打开管理项目对话框。
import { useMemo, useRef } from 'react'
import {
  Ellipsis,
  FolderCog,
  FolderMinus,
  FolderPen,
  FolderPlus,
  Plus,
  RotateCcw,
  Trash2,
} from 'lucide-react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { NO_WORKSPACE_KEY } from '@/features/chat/session-workspaces'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { BoardItem } from './board-model'
import { useBoardViewStore } from './board-view-store'
import { BOARD_FILTER_ID } from './new-task'
import { overviewFilterCounts } from './overview-model'
import { ProjectMenuDialogs } from './ProjectMenuDialogs'
import { useCatalogProjects } from './useCatalogProjects'
import type { BoardRuntimeCapabilities } from './useBoardItems'
import { useProjectMenu } from './useProjectMenu'

const ALL = 'all'
const LANES: readonly BoardLane[] = ['attention', 'running', 'queued', 'done']

export type BoardFilterBarProps = {
  items: readonly BoardItem[]
  now: number
  capabilities: BoardRuntimeCapabilities
  laneName: Record<BoardLane, string>
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  // 添加项目或“在此项目新建任务”选好项目之后进入新任务草稿。
  onNewTask: () => void
}

export function BoardFilterBar({
  items,
  now,
  capabilities,
  laneName,
  notify,
  requestConfirm,
  requestText,
  onNewTask,
}: BoardFilterBarProps) {
  const { t } = useI18n()
  const lanes = useBoardViewStore((state) => state.lanes)
  const projectFilter = useBoardViewStore((state) => state.projects)
  const setLaneFilter = useBoardViewStore((state) => state.setLaneFilter)
  const setProjectFilter = useBoardViewStore((state) => state.setProjectFilter)
  const projects = useCatalogProjects()
  const projectKeys = useMemo(() => projects.map((project) => project.key), [projects])
  const menu = useProjectMenu({ notify, requestConfirm, requestText, onNewTask, keys: projectKeys })
  // 菜单项会打开对话框：等菜单关闭、焦点归还阶段再打开，否则焦点会被菜单拉回触发键。
  const afterClose = useRef<(() => void) | null>(null)
  const counts = useMemo(
    () => overviewFilterCounts(items, { now, capabilities }),
    [capabilities, items, now],
  )
  // 排队只在 Runtime 声明 sessionQueue 且确有排队时出现（规格 2.1、A9），不做假数据。
  const shownLanes = LANES.filter(
    (lane) => lane !== 'queued' || (capabilities.sessionQueue === true && counts.lanes.queued > 0),
  )
  const laneValue: BoardLane | typeof ALL = lanes[0] ?? ALL
  const projectValue = projectFilter[0] ?? ''
  const selected = projects.find((project) => project.key === projectValue)
  const hasFolder = Boolean(selected?.cwd) && selected?.key !== NO_WORKSPACE_KEY
  return (
    <div data-overview-filters className="mb-4 flex min-w-0 flex-wrap items-center gap-2">
      <SegmentedControl
        // “/”聚焦这里当前选中的一段（App 的 focusPageSearch）。
        id={BOARD_FILTER_ID}
        aria-label={t('board:filter.statuses')}
        value={laneValue}
        onValueChange={(value) => setLaneFilter(value === ALL ? [] : [value])}
        options={[
          { value: ALL, label: t('board:sidebar.filter.all'), count: counts.total },
          ...shownLanes.map((lane) => ({
            value: lane,
            label: laneName[lane],
            count: counts.lanes[lane],
          })),
        ]}
      />
      <div className="flex min-w-0 items-center gap-1">
        <AppSelect
          aria-label={t('board:filter.projects')}
          className="w-48"
          value={projectValue}
          onChange={(event) => setProjectFilter(event.target.value ? [event.target.value] : [])}
        >
          <option value="">{t('board:filter.allProjects')}</option>
          {projects.map((project) => (
            <option key={project.key} value={project.key}>
              {project.label}
            </option>
          ))}
        </AppSelect>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              data-overview-project-menu
              aria-label={
                selected
                  ? t('board:project.actionsFor', { project: selected.label })
                  : t('board:sidebar.filter.projectActions')
              }
              title={
                selected
                  ? t('board:project.actionsFor', { project: selected.label })
                  : t('board:sidebar.filter.projectActions')
              }
            >
              <Ellipsis aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            className="w-52"
            onCloseAutoFocus={(event) => {
              const next = afterClose.current
              if (!next) return
              afterClose.current = null
              event.preventDefault()
              next()
            }}
          >
            {selected && (
              <>
                {hasFolder && (
                  <>
                    <DropdownMenuItem
                      onSelect={() => (afterClose.current = () => menu.actions.onNewTask(selected))}
                    >
                      <Plus aria-hidden="true" />
                      {t('board:project.newTaskHere')}
                    </DropdownMenuItem>
                    <DropdownMenuItem onSelect={() => menu.actions.onRename(selected)}>
                      <FolderPen aria-hidden="true" />
                      {t('board:project.rename')}
                    </DropdownMenuItem>
                    {selected.custom && (
                      <DropdownMenuItem onSelect={() => menu.actions.onResetName(selected)}>
                        <RotateCcw aria-hidden="true" />
                        {t('board:project.resetName')}
                      </DropdownMenuItem>
                    )}
                  </>
                )}
                <DropdownMenuItem
                  variant="destructive"
                  disabled={menu.deleting}
                  onSelect={() => menu.actions.onDelete(selected)}
                >
                  {selected.count === 0 ? (
                    <FolderMinus aria-hidden="true" />
                  ) : (
                    <Trash2 aria-hidden="true" />
                  )}
                  {/* 没有会话的登记项目只是取消登记，不叫“删除”（与管理项目对话框一致）。 */}
                  {selected.count === 0 ? t('board:project.remove') : t('board:project.delete')}
                </DropdownMenuItem>
                <DropdownMenuSeparator />
              </>
            )}
            <DropdownMenuItem
              disabled={menu.picking}
              onSelect={() => (afterClose.current = () => void menu.newProject())}
            >
              <FolderPlus aria-hidden="true" />
              {t('board:sidebar.filter.addProject')}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => (afterClose.current = menu.openManage)}>
              <FolderCog aria-hidden="true" />
              {t('board:project.manage')}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <ProjectMenuDialogs {...menu.dialogs} />
    </div>
  )
}
