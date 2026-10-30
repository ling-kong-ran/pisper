// @public 项目（按工作目录分组的会话）的整理操作：删除项目连同其全部会话、改显示名、选新项目目录。
// 看板的“项目 ▾”、项目横带“⋯”与“管理项目”共用这里；确认文案、删除语义与原侧栏一致：
// 删除前重新读取完整目录（不依赖搜索结果或看板筛选），确认框写出完整路径与会话数，
// 逐个删除并只把接口确认成功的计入，失败或无法复核时如实报告剩余数量。
// 只依赖聊天领域的事件、目录规则与偏好，不导入页面，也没有导入副作用。
import type { Notify } from '@/app/route-context'
import { STORAGE_KEYS } from '@/app/storage'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { apiJson } from '@/lib/api'
import { pageStateStorage } from '@/lib/page-state-storage'
import { pickSystemDirectory, type SystemDirectoryPick } from '@/lib/pick-system-directory'
import { fetchStartupQuery } from '@/lib/startup-queries'
import { announceActiveSession, announceSessionsUpdated } from './events'
import {
  deleteSessionsSequentially,
  NO_WORKSPACE_KEY,
  replacementActiveSessionId,
  sessionsInWorkspace,
  type WorkspaceSession,
} from './session-workspaces'
import { useWorkspaceOrderStore } from './workspace-order-store'

type Translate = (key: string, values?: Record<string, unknown>) => string

// 项目的最小描述：key 是 workspaceKey（与 workspace-order-store、看板筛选一致），
// label 是当前显示名（自定义名或目录名），cwd 为空表示“无工作区”分组。
export type WorkspaceProject = { key: string; cwd: string; label: string }

export type ProjectDeleteOutcome =
  'deleted' | 'partial' | 'unverified' | 'empty' | 'cancelled' | 'failed'

export type ProjectActionContext = {
  t: Translate
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
}

// 显示名上限与 workspace-order-store 一致（按码点计）。
export const PROJECT_NAME_MAX_LENGTH = 120

// 危险操作始终带上完整路径：同名目录可能位于不同父目录。
function projectScopeLabel(project: WorkspaceProject) {
  return project.cwd ? `${project.label} · ${project.cwd}` : project.label
}

async function readCatalog() {
  return (await fetchStartupQuery<{ sessions: WorkspaceSession[] }>('sessions', true)).sessions
}

// 被删掉的会话若恰好是记住的“当前会话”，换成剩下的第一个或清空；只改记录，不发起打开请求，
// 避免在看板上排下一个会把人带进会话页的导航意图。本机存储不可用时跳过：删除结果已经确定，
// 不能因为记不住“当前会话”就把整次删除报成失败。
function replaceRememberedSession(
  remaining: readonly WorkspaceSession[],
  deletedIds: ReadonlySet<string>,
  verified: boolean,
) {
  try {
    const current = pageStateStorage.getItem(STORAGE_KEYS.activeSession) || ''
    const nextId = replacementActiveSessionId(current, remaining, deletedIds, verified)
    if (nextId === null) return
    if (nextId) pageStateStorage.setItem(STORAGE_KEYS.activeSession, nextId)
    else pageStateStorage.removeItem(STORAGE_KEYS.activeSession)
    announceActiveSession(nextId)
  } catch {
    // 见上：只影响“当前会话”的记录。
  }
}

/**
 * 删除项目及其全部会话（与原侧栏“删除项目”同一语义）。从不抛错，结果经 notify 告知用户，
 * 返回值供调用方决定后续界面（例如关闭菜单、刷新列表）。
 */
export async function deleteWorkspaceProject(
  project: WorkspaceProject,
  { t, notify, requestConfirm }: Pick<ProjectActionContext, 't' | 'notify' | 'requestConfirm'>,
): Promise<ProjectDeleteOutcome> {
  const label = projectScopeLabel(project)
  try {
    const freshSessions = await readCatalog()
    const targets = sessionsInWorkspace(freshSessions, project.key)
    if (!targets.length) {
      notify(t('navigation:appSidebar.projectNoChats', { project: label }), 'info')
      return 'empty'
    }
    const approved = await requestConfirm({
      title: t('navigation:appSidebar.deleteProject'),
      message: t('navigation:appSidebar.deleteProjectConfirm', {
        project: label,
        count: targets.length,
      }),
      confirmLabel: t('navigation:appSidebar.deleteProjectAction'),
    })
    if (!approved) return 'cancelled'
    const result = await deleteSessionsSequentially(
      targets.map((session) => session.id),
      (id) => apiJson(`/api/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' }),
    )
    announceSessionsUpdated(
      result.deletedIds.length ? { deletedIds: result.deletedIds } : undefined,
    )
    const deletedIds = new Set(result.deletedIds)
    let refreshed: WorkspaceSession[] | null = null
    try {
      refreshed = await readCatalog()
    } catch {
      // 删除结果已经确定；目录暂时读不到时不能把项目报成已完全删除。
    }
    const remaining = refreshed ?? freshSessions.filter((session) => !deletedIds.has(session.id))
    replaceRememberedSession(remaining, deletedIds, refreshed !== null)
    if (refreshed === null) {
      notify(
        t('navigation:appSidebar.projectDeleteUnverified', {
          project: label,
          deleted: result.deletedIds.length,
        }),
        'error',
      )
      return 'unverified'
    }
    const remainingCount = sessionsInWorkspace(refreshed, project.key).length
    if (result.failedId !== null || remainingCount) {
      notify(
        t('navigation:appSidebar.projectDeletePartial', {
          project: label,
          deleted: result.deletedIds.length,
          remaining: remainingCount,
        }),
        'error',
      )
      return 'partial'
    }
    notify(t('navigation:appSidebar.projectDeleted', { project: label }))
    return 'deleted'
  } catch (error) {
    notify(error instanceof Error ? error.message : String(error), 'error')
    return 'failed'
  }
}

function saveProjectName(
  project: WorkspaceProject,
  name: string,
  { t, notify }: Pick<ProjectActionContext, 't' | 'notify'>,
) {
  try {
    useWorkspaceOrderStore.getState().setWorkspaceName(project.cwd, name)
    notify(t('navigation:appSidebar.projectNameUpdated'))
    return true
  } catch {
    notify(t('navigation:appSidebar.projectNameSaveFailed'), 'error')
    return false
  }
}

/**
 * 改项目的显示名：只改本机偏好（workspace-order-store），不动实际目录与会话。返回是否改动。
 * 改回目录名本身时清掉自定义名，免得目录以后改名、显示名还停在旧值。
 */
export async function renameWorkspaceProject(
  project: WorkspaceProject,
  defaultLabel: string,
  context: Pick<ProjectActionContext, 't' | 'notify' | 'requestText'>,
  copy: { description: string },
) {
  // “无工作区”分组没有目录可以挂名字。
  if (!project.cwd || project.key === NO_WORKSPACE_KEY) return false
  const { t, requestText } = context
  const name = await requestText({
    title: t('navigation:appSidebar.renameProject'),
    message: copy.description,
    inputLabel: t('navigation:appSidebar.projectName'),
    value: project.label,
    maxLength: PROJECT_NAME_MAX_LENGTH,
    confirmLabel: t('chat:chatHistoryPage.save'),
  })
  const next = name?.trim() || ''
  if (!next) return false
  const stored = next === defaultLabel ? '' : next
  if (stored === (useWorkspaceOrderStore.getState().names[project.key] || '')) return false
  return saveProjectName(project, stored, context)
}

// 恢复目录名：清掉自定义显示名。
export function resetWorkspaceProjectName(
  project: WorkspaceProject,
  context: Pick<ProjectActionContext, 't' | 'notify'>,
) {
  if (!project.cwd || !useWorkspaceOrderStore.getState().names[project.key]) return false
  return saveProjectName(project, '', context)
}

/**
 * 选择新项目的目录：先走系统文件管理器；系统对话框确实无法出现（配对设备、无图形会话、手机 Runtime）
 * 时返回 unavailable，由调用方打开 WorkspacePicker 手动填写 Runtime 路径——与原侧栏同一流程。
 */
export function pickProjectDirectory(t: Translate): Promise<SystemDirectoryPick> {
  return pickSystemDirectory('', t('common:workspacePicker.setWorkingDirectory'))
}
