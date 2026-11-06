// 侧栏里的项目列表（会话页的“项目”分区、全部代理页的项目筛选）共用的项目入口，项目列表见 useCatalogProjects。
// 每个项目的 ⋯ 直接给出在此项目新建任务、重命名、恢复目录名、上移 / 下移、删除或移除，
// 不必先打开“管理项目”对话框；选目录器与管理项目对话框的状态在这里，调用方用 ProjectMenuDialogs 把它们放进树里。
import { useMemo, useState } from 'react'
import type { Notify } from '@/app/route-context'
import { useWorkspaceOrderStore } from '@/features/chat/workspace-order-store'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { launchChoiceForProject, useLaunchDefaultsStore } from './launch-defaults-store'
import type { ProjectSectionActions } from './ProjectSectionMenu'
import { useProjectCommands } from './useProjectCommands'

export type ProjectMenuOptions = {
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  // 选好项目（新建项目或“在此项目新建任务”）之后进入新任务草稿。
  onNewTask: () => void
  // 列表里看得见的项目键：上移 / 下移只在它们之间换位。
  keys: readonly string[]
}

export function useProjectMenu({
  notify,
  requestConfirm,
  requestText,
  onNewTask,
  keys,
}: ProjectMenuOptions) {
  const commands = useProjectCommands({
    notify,
    requestConfirm,
    requestText,
    onProjectChosen: onNewTask,
    announceChoice: true,
  })
  const moveWorkspace = useWorkspaceOrderStore((state) => state.moveWorkspace)
  const [manageMounted, setManageMounted] = useState(false)
  const [manageOpen, setManageOpen] = useState(false)
  const openManage = () => {
    setManageMounted(true)
    setManageOpen(true)
  }
  const { rename, resetName, remove, unregister } = commands
  const actions = useMemo<ProjectSectionActions>(
    () => ({
      onNewTask: (project) => {
        useLaunchDefaultsStore.getState().rememberLaunch(launchChoiceForProject(project.cwd))
        onNewTask()
      },
      onRename: (project) => void rename(project),
      onResetName: (project) => resetName(project),
      onMove: (project, offset) => moveWorkspace(project.key, offset, keys),
      onManage: () => {
        setManageMounted(true)
        setManageOpen(true)
      },
      // 没有会话的项目只是登记过：移除只取消登记，不弹确认；有会话的才连同会话一起删除（有确认框）。
      onDelete: (project) => (project.count === 0 ? unregister(project) : void remove(project)),
    }),
    [keys, moveWorkspace, onNewTask, remove, rename, resetName, unregister],
  )
  return {
    actions,
    deleting: Boolean(commands.busyKey),
    newProject: commands.newProject,
    picking: commands.picking,
    openManage,
    dialogs: { commands, manageMounted, manageOpen, onManageOpenChange: setManageOpen },
  }
}

export type ProjectMenuDialogsProps = ReturnType<typeof useProjectMenu>['dialogs']
