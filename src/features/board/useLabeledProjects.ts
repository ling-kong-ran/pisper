// 看板上的项目（全部代理的按项目分组、手机筛选）：按 workspace-order-store 记住的顺序排好，带显示名。
// 还没有会话（或今天没有代理）的登记项目也列进来，计 0 个；显示名优先用户自定义，否则取目录名。
import { useMemo } from 'react'
import { useI18n } from '@/app/use-i18n'
import { useRegisteredProjectsStore } from '@/features/chat/registered-projects-store'
import { useWorkspaceOrderStore } from '@/features/chat/workspace-order-store'
import { sortProjectsByOrder, type BoardProject } from './board-model'
import { withRegisteredProjects } from './board-projects'
import { projectLabel } from './card-presentation'

export type LabeledProject = BoardProject & {
  label: string
  // 显示名是用户自定义的（可以恢复目录名）。
  custom: boolean
}

export function useLabeledProjects(projects: readonly BoardProject[]): LabeledProject[] {
  const { t } = useI18n()
  const names = useWorkspaceOrderStore((state) => state.names)
  const order = useWorkspaceOrderStore((state) => state.order)
  const registered = useRegisteredProjectsStore((state) => state.projects)
  return useMemo(
    () =>
      sortProjectsByOrder(withRegisteredProjects([...projects], registered), order).map(
        (project) => ({
          ...project,
          label:
            names[project.key] ||
            (project.cwd ? projectLabel(project.cwd) : t('board:filter.noProject')),
          custom: Boolean(project.cwd && names[project.key]),
        }),
      ),
    [names, order, projects, registered, t],
  )
}
