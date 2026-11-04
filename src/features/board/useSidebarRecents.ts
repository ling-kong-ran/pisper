// 侧栏“最近”的数据：状态组（需要你、运行中、排队）之外的未归档会话（sidebar-recents）。
// 侧栏渲染与 ⌥↑ / ⌥↓ 的切换顺序（SidebarCommands）共用，保证键盘顺序就是看得见的顺序。
import { useMemo } from 'react'
import type { SessionSummary } from '@/types/chat'
import type { BoardItem } from './board-model'
import type { SidebarModel } from './sidebar-model'
import { buildSidebarRecents } from './sidebar-recents'

// 侧栏常驻的状态组：需要你、运行中一直在（为 0 时只留标题），排队只在有排队时；
// 置顶与今天完成不单列，归进“最近”（置顶排在最前）。
export function sidebarStatusGroups(model: SidebarModel) {
  return model.groups.filter(
    (group) =>
      group.id === 'attention' ||
      group.id === 'running' ||
      (group.id === 'queued' && group.count > 0),
  )
}

export function useSidebarRecents(
  model: SidebarModel,
  items: readonly BoardItem[],
  sessions: readonly SessionSummary[] | undefined,
  currentId: string,
) {
  const shownIds = useMemo(
    () => new Set(sidebarStatusGroups(model).flatMap((group) => group.rows.map((row) => row.id))),
    [model],
  )
  return useMemo(
    () => buildSidebarRecents(sessions ?? [], { items, shownIds, currentId }),
    [currentId, items, sessions, shownIds],
  )
}
