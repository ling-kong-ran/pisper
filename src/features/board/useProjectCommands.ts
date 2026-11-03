// 看板上的项目入口编排：新建项目（选目录 → 登记为项目并选为启动台的派发目标，见 useNewProjectPicker）、
// 改名、恢复目录名、删除项目（连同会话）与移除没有会话的登记项目。
// 规则与确认文案来自聊天领域的 project-actions（与原侧栏同一语义）；这里只持有界面状态：
// 正在删除的项目，以及新建项目的提示方式。
import { useRef, useState } from 'react'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import {
  deleteWorkspaceProject,
  removeRegisteredProject,
  renameWorkspaceProject,
  resetWorkspaceProjectName,
  type WorkspaceProject,
} from '@/features/chat/project-actions'
import { workspaceKey } from '@/features/chat/session-workspaces'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { useBoardViewStore } from './board-view-store'
import { projectLabel } from './card-presentation'
import { useLaunchDefaultsStore } from './launch-defaults-store'
import { useNewProjectPicker } from './useNewProjectPicker'

export type ProjectCommandsOptions = {
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  // 新目录已经选为派发目标之后：桌面聚焦启动台，手机打开派发底部表单。
  onProjectChosen: () => void
  // 是否用提示告知“已选择项目”：手机上紧接着打开的底部表单已经写出项目，提示反而会盖住派发键。
  announceChoice: boolean
}

export function useProjectCommands({
  notify,
  requestConfirm,
  requestText,
  onProjectChosen,
  announceChoice,
}: ProjectCommandsOptions) {
  const { t } = useI18n()
  const [busyKey, setBusyKey] = useState('')
  const busyRef = useRef('')
  const { newProject, picking, picker } = useNewProjectPicker({
    onProjectChosen,
    onError: (message) => notify(message, 'error'),
    onRemembered: announceChoice
      ? (path) => notify(t('board:project.chosen', { project: projectLabel(path) }), 'info')
      : undefined,
  })

  const context = { t, notify, requestConfirm, requestText }

  const rename = (project: WorkspaceProject) =>
    renameWorkspaceProject(project, projectLabel(project.cwd), context, {
      description: t('board:project.renameDescription'),
    })

  const resetName = (project: WorkspaceProject) => resetWorkspaceProjectName(project, context)

  // 删掉或移除的项目不应继续是启动台记住的派发目标，也不应留在项目筛选里（否则看板只剩
  // “筛选后为空”，筛选按钮上还显示一个已经不存在的路径）。
  const forgetProject = (project: WorkspaceProject) => {
    const defaults = useLaunchDefaultsStore.getState()
    if (defaults.cwd && workspaceKey(defaults.cwd) === project.key)
      defaults.rememberLaunch({ cwd: '' })
    const view = useBoardViewStore.getState()
    if (view.projects.includes(project.key))
      view.setProjectFilter(view.projects.filter((key) => key !== project.key))
  }

  // 删除项目连同其全部会话（有确认框）；成功后 project-actions 同时取消登记。
  const remove = async (project: WorkspaceProject) => {
    if (busyRef.current) return
    busyRef.current = project.key
    setBusyKey(project.key)
    try {
      const outcome = await deleteWorkspaceProject(project, context)
      // 部分删除时项目还在，保留选择。
      if (outcome === 'deleted') forgetProject(project)
    } finally {
      busyRef.current = ''
      setBusyKey('')
    }
  }

  // 移除没有会话的登记项目：只取消登记，不删任何东西，所以不弹确认框。
  const unregister = (project: WorkspaceProject) => {
    if (busyRef.current) return
    if (removeRegisteredProject(project, context)) forgetProject(project)
  }

  return { newProject, picking, rename, resetName, remove, unregister, busyKey, picker }
}

export type ProjectCommands = ReturnType<typeof useProjectCommands>
