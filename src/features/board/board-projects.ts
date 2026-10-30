// 看板各处项目列表的合并规则：会话目录或看板里出现的项目，补上还没有会话的登记项目
// （会话数记 0），按 workspaceKey 去重；顺序与显示名仍由 workspace-order-store 决定。
// 纯函数，不读 store：调用方把登记列表（registered-projects-store）与偏好传进来。
import { unlistedRegisteredProjects } from '@/features/chat/registered-projects'
import {
  groupSessionsByWorkspace,
  NO_WORKSPACE_KEY,
  type WorkspaceSession,
} from '@/features/chat/session-workspaces'
import { sortProjectsByOrder } from './board-model'
import { projectLabel } from './card-presentation'

export type CountedProject = { key: string; cwd: string; count: number }

// 项目筛选与“按项目分组”的候选：看板上的项目在前，没有出现过的登记项目补在后面（0 张卡）。
export function withRegisteredProjects<T extends CountedProject>(
  projects: readonly T[],
  registered: readonly string[],
): (T | CountedProject)[] {
  const unlisted = unlistedRegisteredProjects(
    registered,
    projects.map((project) => project.key),
  ).map((project) => ({ ...project, count: 0 }))
  return [...projects, ...unlisted]
}

export type LaunchProject = { key: string; cwd: string; label: string }

export const MAX_RECENT_PROJECTS = 12

type RecentSession = WorkspaceSession & { modified?: string }

/**
 * 启动台“最近的项目”：还没有会话的登记项目排最前（刚新建、还没用过），其后是按会话修改时间
 * 倒序去重的项目（归档会话所在目录也算，它们仍是用户的项目），后者最多 MAX_RECENT_PROJECTS 个。
 */
export function recentLaunchProjects(
  sessions: readonly RecentSession[],
  registered: readonly string[],
  names: Readonly<Record<string, string>>,
): LaunchProject[] {
  const ordered = [...sessions].sort(
    (left, right) =>
      (Date.parse(right.modified || '') || 0) - (Date.parse(left.modified || '') || 0),
  )
  const groups = groupSessionsByWorkspace(ordered).filter(
    (group) => group.key !== NO_WORKSPACE_KEY && group.cwd,
  )
  const unlisted = unlistedRegisteredProjects(
    registered,
    groups.map((group) => group.key),
  )
  return [...unlisted, ...groups.slice(0, MAX_RECENT_PROJECTS)].map((project) => ({
    key: project.key,
    cwd: project.cwd,
    label: names[project.key] || projectLabel(project.cwd),
  }))
}

/**
 * 没有记住派发目标时的隐式默认目录：最近有会话的项目优先（派发目标仓库最不能看错，一个早就登记、
 * 从没用过的目录不该因为排在列表最前就成了默认）；没有任何会话项目时才取第一个登记项目。
 * 列表顺序（登记的空项目在前）不变，只影响默认选中。
 */
export function defaultLaunchCwd(
  sessions: readonly RecentSession[],
  projects: readonly LaunchProject[],
) {
  return recentLaunchProjects(sessions, [], {})[0]?.cwd || projects[0]?.cwd || ''
}

export type ManagedProject = CountedProject & {
  label: string
  folderLabel: string
  custom: boolean
}

// 管理项目对话框的列表：完整会话目录里的项目，补上没有会话的登记项目，再按记住的顺序排列。
export function managedProjects(
  sessions: readonly WorkspaceSession[],
  registered: readonly string[],
  names: Readonly<Record<string, string>>,
  order: readonly string[],
  noProjectLabel: string,
): ManagedProject[] {
  const groups = groupSessionsByWorkspace(sessions).map((group) => ({
    key: group.key,
    cwd: group.cwd,
    count: group.sessions.length,
  }))
  return sortProjectsByOrder(
    withRegisteredProjects(groups, registered).map((project) => {
      const folderLabel = project.cwd ? projectLabel(project.cwd) : noProjectLabel
      const custom = project.key !== NO_WORKSPACE_KEY ? names[project.key] || '' : ''
      return { ...project, label: custom || folderLabel, folderLabel, custom: Boolean(custom) }
    }),
    order,
  )
}
