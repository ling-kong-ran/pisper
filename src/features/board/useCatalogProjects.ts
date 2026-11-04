// 侧栏项目列表与“管理项目”对话框共用的项目来源：完整会话目录（含前几天的、只剩归档会话的项目）
// 加还没有会话的登记项目，不再只取今天看板上出现的项目——否则管理项目里有两个项目，侧栏却只列出一个。
import { useMemo } from 'react'
import { useI18n } from '@/app/use-i18n'
import { useRegisteredProjectsStore } from '@/features/chat/registered-projects-store'
import { useWorkspaceOrderStore } from '@/features/chat/workspace-order-store'
import { managedProjects, type ManagedProject } from './board-projects'
import { useSessionListQuery } from './useSessionListQuery'

type CatalogSession = { id: string; cwd?: string; projectless?: boolean; projectCwd?: string }

// 全部项目，按记住的顺序排好、带显示名；count 是完整目录里的会话数（含归档），0 表示只是登记过。
export function useCatalogProjects(): ManagedProject[] {
  const { t } = useI18n()
  const catalog = useSessionListQuery<{ sessions: CatalogSession[] }>()
  const order = useWorkspaceOrderStore((state) => state.order)
  const names = useWorkspaceOrderStore((state) => state.names)
  const registered = useRegisteredProjectsStore((state) => state.projects)
  return useMemo(
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
}
