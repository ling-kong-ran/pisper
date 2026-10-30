// 管理项目：列出完整会话目录里的全部项目（含只剩归档会话的）与还没有会话的登记项目，
// 可以调整顺序、改显示名、恢复目录名，以及删除或移除项目。顺序与显示名写进 workspace-order-store，
// 看板页头、按项目分组与启动台共用。有会话的项目“连同会话一起删除”沿用原侧栏的确认文案与接口
// （经 useProjectCommands → project-actions）；没有会话的登记项目只是“移除项目”（取消登记）。
import { useEffect, useMemo, useRef, useState } from 'react'
import {
  ArrowDown,
  ArrowUp,
  FolderMinus,
  FolderPlus,
  Pencil,
  RotateCcw,
  Trash2,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { OrbitSpinner } from '@/components/ui/spinner'
import { useRegisteredProjectsStore } from '@/features/chat/registered-projects-store'
import { NO_WORKSPACE_KEY } from '@/features/chat/session-workspaces'
import { useWorkspaceOrderStore } from '@/features/chat/workspace-order-store'
import { cn } from '@/lib/utils'
import { managedProjects, type ManagedProject } from './board-projects'
import { shortPath } from './card-presentation'
import type { ProjectCommands } from './useProjectCommands'
import { useSessionListQuery } from './useSessionListQuery'

type CatalogSession = { id: string; cwd?: string }

type ManageProjectsDialogProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  commands: ProjectCommands
}

// 行内图标键：桌面 28px，粗指针与手机外壳放大到 44px 触控目标。
const ROW_ICON_BUTTON = 'flex-none pointer-coarse:size-11 in-data-[phone-shell]:size-11'

export function ManageProjectsDialog({ open, onOpenChange, commands }: ManageProjectsDialogProps) {
  const { t } = useI18n()
  const catalog = useSessionListQuery<{ sessions: CatalogSession[] }>()
  const order = useWorkspaceOrderStore((state) => state.order)
  const names = useWorkspaceOrderStore((state) => state.names)
  const registered = useRegisteredProjectsStore((state) => state.projects)
  const rememberWorkspaces = useWorkspaceOrderStore((state) => state.rememberWorkspaces)
  const moveWorkspace = useWorkspaceOrderStore((state) => state.moveWorkspace)
  const listRef = useRef<HTMLUListElement>(null)
  const doneRef = useRef<HTMLButtonElement>(null)
  // 上移 / 下移后行的 DOM 会被搬动，焦点要跟着回到同一行的同一个按钮（到边界时换成另一个方向）。
  const [focusTarget, setFocusTarget] = useState<{ key: string; offset: -1 | 1 } | null>(null)
  // 删除或移除一行后，它的按钮随行消失：焦点交给补上来的相邻行的同一个键，没有了就交给“完成”。
  const [removedFocus, setRemovedFocus] = useState<{ key: string; next: string } | null>(null)

  const projects = useMemo(
    () =>
      managedProjects(
        catalog.data?.sessions ?? [],
        registered,
        names,
        order,
        t('board:filter.noProject'),
      ),
    [catalog.data, names, order, registered, t],
  )
  const keys = useMemo(() => projects.map((project) => project.key), [projects])

  // 打开时把当前看到的顺序写进偏好：之后的上移 / 下移与看板页头都以它为准。
  useEffect(() => {
    if (open && keys.length) rememberWorkspaces(keys)
  }, [keys, open, rememberWorkspaces])

  useEffect(() => {
    if (!focusTarget) return
    const list = listRef.current
    const find = (offset: -1 | 1) =>
      list?.querySelector<HTMLButtonElement>(
        `[data-project-move="${offset}"][data-project-key="${CSS.escape(focusTarget.key)}"]`,
      )
    const preferred = find(focusTarget.offset)
    ;(preferred && !preferred.disabled
      ? preferred
      : find(focusTarget.offset === -1 ? 1 : -1)
    )?.focus()
    setFocusTarget(null)
  }, [focusTarget, projects])

  useEffect(() => {
    if (!removedFocus) return
    setRemovedFocus(null)
    // 行还在（取消、失败或只删了一部分）：焦点留在原处。
    if (projects.some((project) => project.key === removedFocus.key)) return
    const next = removedFocus.next
      ? listRef.current?.querySelector<HTMLButtonElement>(
          `[data-project-remove][data-project-key="${CSS.escape(removedFocus.next)}"]`,
        )
      : null
    ;(next ?? doneRef.current)?.focus()
  }, [projects, removedFocus])

  const move = (project: ManagedProject, offset: -1 | 1) => {
    moveWorkspace(project.key, offset, keys)
    setFocusTarget({ key: project.key, offset })
  }

  const removeRow = async (project: ManagedProject, index: number) => {
    const next = projects[index + 1]?.key ?? projects[index - 1]?.key ?? ''
    if (project.count === 0) commands.unregister(project)
    else await commands.remove(project)
    setRemovedFocus({ key: project.key, next })
  }

  const deleting = Boolean(commands.busyKey)
  let body
  if (catalog.isPending)
    body = (
      <p role="status" className="flex items-center gap-2 py-6 text-sm text-content-muted">
        <OrbitSpinner className="size-4" />
        {t('board:project.loading')}
      </p>
    )
  else if (catalog.error && !catalog.data)
    body = (
      <div role="alert" className="flex flex-col items-start gap-2 py-6 text-sm">
        <p className="text-content">{t('board:project.error')}</p>
        <Button variant="secondary" size="sm" onClick={() => void catalog.refetch()}>
          {t('board:page.retry')}
        </Button>
      </div>
    )
  else if (projects.length === 0)
    body = (
      <div className="flex flex-col gap-1 py-6 text-sm">
        <p className="font-medium text-content">{t('board:project.empty')}</p>
        <p className="text-content-muted">{t('board:project.emptyHint')}</p>
      </div>
    )
  else
    body = (
      <ul
        ref={listRef}
        aria-label={t('board:project.listLabel')}
        className="flex flex-col divide-y divide-stroke-soft"
        data-manage-projects
      >
        {projects.map((project, index) => {
          const busy = commands.busyKey === project.key
          const named = project.cwd && project.key !== NO_WORKSPACE_KEY
          // 没有会话的项目只能来自登记：移除只是取消登记；有会话的才是“连同会话一起删除”。
          const removeOnly = project.count === 0
          const removeLabel = removeOnly
            ? t('board:project.removeNamed', { project: project.label })
            : t('board:project.deleteWithChatsNamed', { project: project.label })
          return (
            <li
              key={project.key}
              data-project-row={project.key}
              data-project-registered-only={removeOnly || undefined}
              aria-busy={busy || undefined}
              className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 py-2"
            >
              <span
                aria-hidden="true"
                className="grid size-7 flex-none place-items-center rounded-sm border border-stroke-strong bg-solid font-mono text-xs font-semibold text-content uppercase"
              >
                {Array.from(project.label)[0] || '·'}
              </span>
              <div className="flex min-w-0 flex-1 flex-col">
                <span className="truncate font-mono text-sm font-semibold text-content">
                  {project.label}
                </span>
                <span
                  className="truncate font-mono text-xs text-content-muted"
                  title={project.cwd || undefined}
                >
                  {project.custom
                    ? `${t('board:project.folderName', { name: project.folderLabel })} · ${shortPath(project.cwd)}`
                    : project.cwd
                      ? shortPath(project.cwd)
                      : ''}
                </span>
              </div>
              <span className="flex-none text-xs text-content-muted tabular-nums max-[480px]:hidden">
                {busy
                  ? t('board:project.deleting')
                  : t('board:project.sessions', { count: project.count })}
              </span>
              {/* 窄屏（手机）上操作键换到第二行靠右，名称不被 44px 触控键挤没。 */}
              <span className="flex flex-none items-center max-[480px]:w-full max-[480px]:justify-end">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className={ROW_ICON_BUTTON}
                  data-project-move="-1"
                  data-project-key={project.key}
                  disabled={index === 0}
                  aria-label={t('board:project.moveUp', { project: project.label })}
                  title={t('board:project.moveUp', { project: project.label })}
                  onClick={() => move(project, -1)}
                >
                  <ArrowUp aria-hidden="true" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className={ROW_ICON_BUTTON}
                  data-project-move="1"
                  data-project-key={project.key}
                  disabled={index === projects.length - 1}
                  aria-label={t('board:project.moveDown', { project: project.label })}
                  title={t('board:project.moveDown', { project: project.label })}
                  onClick={() => move(project, 1)}
                >
                  <ArrowDown aria-hidden="true" />
                </Button>
                {named && (
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className={ROW_ICON_BUTTON}
                    aria-label={t('board:project.renameNamed', { project: project.label })}
                    title={t('board:project.renameNamed', { project: project.label })}
                    onClick={() => void commands.rename(project)}
                  >
                    <Pencil aria-hidden="true" />
                  </Button>
                )}
                {named && project.custom && (
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className={ROW_ICON_BUTTON}
                    aria-label={t('board:project.resetNamed', { project: project.label })}
                    title={t('board:project.resetNamed', { project: project.label })}
                    onClick={() => commands.resetName(project)}
                  >
                    <RotateCcw aria-hidden="true" />
                  </Button>
                )}
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className={cn(ROW_ICON_BUTTON, !removeOnly && 'text-danger hover:text-danger')}
                  disabled={deleting}
                  data-project-remove={removeOnly ? 'unregister' : 'delete'}
                  data-project-key={project.key}
                  aria-label={removeLabel}
                  title={removeLabel}
                  onClick={() => void removeRow(project, index)}
                >
                  {busy ? (
                    <OrbitSpinner className="size-4" />
                  ) : removeOnly ? (
                    <FolderMinus aria-hidden="true" />
                  ) : (
                    <Trash2 aria-hidden="true" />
                  )}
                </Button>
              </span>
            </li>
          )
        })}
      </ul>
    )

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        size="wide"
        scrollMode="body"
        aria-describedby="board-manage-projects"
        // Radix 默认聚焦第一个可用控件（第一行的“下移”）：打开后立刻按回车 / 空格会改掉项目顺序。
        // 改为聚焦“完成”，回车直接关闭，顺序键要用户主动移过去。
        onOpenAutoFocus={(event) => {
          event.preventDefault()
          doneRef.current?.focus()
        }}
      >
        <DialogHeader>
          <DialogTitle>{t('board:project.manageTitle')}</DialogTitle>
          <DialogDescription id="board-manage-projects">
            {t('board:project.manageDescription')}
          </DialogDescription>
        </DialogHeader>
        <DialogBody className="-mx-1 px-1">{body}</DialogBody>
        <DialogFooter>
          <Button
            variant="outline"
            size="dialogAction"
            disabled={commands.picking}
            onClick={() => {
              // 选目录后焦点要交给启动台：先关掉本对话框，免得它在关闭时把焦点抢回来。
              onOpenChange(false)
              void commands.newProject()
            }}
          >
            <FolderPlus aria-hidden="true" />
            {t('board:project.new')}
          </Button>
          <Button ref={doneRef} size="dialogAction" onClick={() => onOpenChange(false)}>
            {t('board:project.done')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
