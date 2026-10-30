// @public 登记的项目（空项目）：经“新建项目…”选过目录、但可能还没有任何会话的项目。
// 有会话的项目本来就由会话目录给出；这里只补上“还没会话”的那部分，各个项目列表再按
// workspaceKey 把两者合并去重（registered-projects 的 unlistedRegisteredProjects）。
// 只是本机界面偏好：经 page-state-storage 落盘并同步到 Runtime 的浏览器偏好文件
// （键已登记在 shared/browser-preferences.mjs），不改 Runtime 协议，也不新建空会话。
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import { pageStateStorage } from '@/lib/page-state-storage'
import {
  normalizeRegisteredProjects,
  registeredProjectPath,
  registerProjectPath,
  unregisterProjectPath,
} from './registered-projects'
import { workspaceKey } from './session-workspaces'

export const REGISTERED_PROJECTS_PREFERENCE_KEY = 'pisper-registered-projects'

type RegisteredProjectsPreferences = { projects: string[] }

type RegisteredProjectsState = RegisteredProjectsPreferences & {
  // 返回保存下来的规范化目录；目录非法时返回空串且不登记。
  registerProject: (cwd: string) => string
  // 返回是否真的取消了一项登记。
  unregisterProject: (cwd: string) => boolean
}

// 隐私模式、配额或 WebView 策略可能禁用存储：写不进去时仍保留本次页面里的登记，不打断交互。
const storage = createJSONStorage<RegisteredProjectsPreferences>(() => ({
  getItem: (name) => {
    try {
      return pageStateStorage.getItem(name)
    } catch {
      return null
    }
  },
  setItem: (name, value) => {
    try {
      pageStateStorage.setItem(name, value)
    } catch {
      // 见上。
    }
  },
  removeItem: (name) => {
    try {
      pageStateStorage.removeItem(name)
    } catch {
      // 存储不可用时没有可清理的持久化状态。
    }
  },
}))

function sameProjects(left: readonly string[], right: readonly string[]) {
  return left.length === right.length && left.every((item, index) => item === right[index])
}

export const useRegisteredProjectsStore = create<RegisteredProjectsState>()(
  persist(
    (set, get) => ({
      projects: [],
      registerProject: (cwd) => {
        const path = registeredProjectPath(cwd)
        if (!path) return ''
        const current = get().projects
        const projects = registerProjectPath(current, path)
        if (!sameProjects(projects, current)) set({ projects })
        return path
      },
      unregisterProject: (cwd) => {
        const current = get().projects
        if (!current.some((item) => workspaceKey(item) === workspaceKey(cwd))) return false
        set({ projects: unregisterProjectPath(current, cwd) })
        return true
      },
    }),
    {
      name: REGISTERED_PROJECTS_PREFERENCE_KEY,
      storage,
      version: 1,
      partialize: ({ projects }) => ({ projects }),
      // 持久化内容来自磁盘或其他端口，按不可信数据重新校验后再合并。
      merge: (persisted, current) => ({
        ...current,
        projects: normalizeRegisteredProjects(
          persisted && typeof persisted === 'object' && 'projects' in persisted
            ? persisted.projects
            : undefined,
        ),
      }),
    },
  ),
)
