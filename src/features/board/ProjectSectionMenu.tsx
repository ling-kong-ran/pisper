// 全部代理“按项目”分区标题上的 ⋯（规格 3.2）：在此项目新建任务、重命名、恢复目录名、上移 / 下移（排序）、
// 管理项目…、删除项目。规则与确认文案沿用 useProjectCommands → project-actions，顺序写进 workspace-order-store。
import { useRef } from 'react'
import {
  ArrowDown,
  ArrowUp,
  Ellipsis,
  FolderCog,
  FolderPen,
  Plus,
  RotateCcw,
  Trash2,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import type { LabeledProject } from './useLabeledProjects'

export type ProjectSectionActions = {
  onNewTask: (project: LabeledProject) => void
  onRename: (project: LabeledProject) => void
  onResetName: (project: LabeledProject) => void
  onMove: (project: LabeledProject, offset: -1 | 1) => void
  onManage: () => void
  onDelete: (project: LabeledProject) => void
}

export function ProjectSectionMenu({
  project,
  first,
  last,
  deleting,
  actions,
}: {
  project: LabeledProject
  first: boolean
  last: boolean
  // 删除进行中时所有“删除项目…”都不可用，避免并发删除。
  deleting: boolean
  actions: ProjectSectionActions
}) {
  const { t } = useI18n()
  // 新任务与管理项目都会打开对话框：等菜单关闭、焦点归还阶段再打开，否则焦点会被菜单拉回触发键。
  const afterClose = useRef<(() => void) | null>(null)
  const hasFolder = Boolean(project.cwd)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className="flex-none"
          aria-label={t('board:project.actionsFor', { project: project.label })}
          title={t('board:project.actionsFor', { project: project.label })}
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
        {hasFolder && (
          <>
            <DropdownMenuItem
              onSelect={() => (afterClose.current = () => actions.onNewTask(project))}
            >
              <Plus aria-hidden="true" />
              {t('board:project.newTaskHere')}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => actions.onRename(project)}>
              <FolderPen aria-hidden="true" />
              {t('board:project.rename')}
            </DropdownMenuItem>
            {project.custom && (
              <DropdownMenuItem onSelect={() => actions.onResetName(project)}>
                <RotateCcw aria-hidden="true" />
                {t('board:project.resetName')}
              </DropdownMenuItem>
            )}
          </>
        )}
        <DropdownMenuItem disabled={first} onSelect={() => actions.onMove(project, -1)}>
          <ArrowUp aria-hidden="true" />
          {t('board:project.moveUpShort')}
        </DropdownMenuItem>
        <DropdownMenuItem disabled={last} onSelect={() => actions.onMove(project, 1)}>
          <ArrowDown aria-hidden="true" />
          {t('board:project.moveDownShort')}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => (afterClose.current = actions.onManage)}>
          <FolderCog aria-hidden="true" />
          {t('board:project.manage')}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          variant="destructive"
          disabled={deleting}
          onSelect={() => actions.onDelete(project)}
        >
          <Trash2 aria-hidden="true" />
          {t('board:project.delete')}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
