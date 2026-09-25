// @public 侧栏组合边界使用的目录顺序和显示名称偏好，不修改实际目录或服务端会话。
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import {
  NO_WORKSPACE_KEY,
  normalizeWorkspaceOrder,
  reconcileWorkspaceOrder,
  workspaceKey,
} from '@/features/chat/session-workspaces'

type WorkspacePreferences = {
  order: string[]
  names: Record<string, string>
}

type WorkspaceOrderState = WorkspacePreferences & {
  rememberWorkspaces: (keys: readonly string[]) => void
  setWorkspaceName: (cwd: string, name: string) => void
}

function validWorkspaceKey(cwd: string) {
  const key = workspaceKey(cwd)
  return key && key !== NO_WORKSPACE_KEY && !key.includes('\0') ? key : null
}

function normalizeWorkspaceNames(value: unknown): Record<string, string> {
  // 目录名可以恰好是 __proto__ 或 toString，映射不能继承 Object.prototype。
  const names: Record<string, string> = Object.create(null)
  if (!value || typeof value !== 'object' || Array.isArray(value)) return names
  for (const [cwd, valueName] of Object.entries(value)) {
    const key = validWorkspaceKey(cwd)
    if (!key || typeof valueName !== 'string') continue
    const name = valueName.trim()
    if (name && Array.from(name).length <= 120) names[key] = name
  }
  return names
}

const storage = createJSONStorage<WorkspacePreferences>(() => window.localStorage)

export const useWorkspaceOrderStore = create<WorkspaceOrderState>()(
  persist(
    (set, get) => ({
      order: [],
      names: normalizeWorkspaceNames(undefined),
      rememberWorkspaces: (keys) => {
        const current = get().order
        const order = reconcileWorkspaceOrder(current, keys)
        if (order.length === current.length && order.every((key, index) => key === current[index]))
          return
        set({ order })
      },
      setWorkspaceName: (cwd, value) => {
        const key = typeof cwd === 'string' ? validWorkspaceKey(cwd) : null
        if (!key || typeof value !== 'string') throw new TypeError('Invalid workspace name')
        const name = value.trim()
        if (Array.from(name).length > 120) throw new RangeError('Workspace name is too long')
        const current = get().names
        if ((current[key] || '') === name) return
        if (!storage) throw new Error('Workspace preferences storage is unavailable')
        const names = normalizeWorkspaceNames(current)
        if (name) names[key] = name
        else delete names[key]
        try {
          set({ names })
        } catch (error) {
          // persist 先改内存后写磁盘；写入失败要恢复名称，避免界面显示未保存的改名。
          try {
            set({ names: current })
          } catch {
            // 恢复内存已经完成，保留第一次写入的错误交给界面展示。
          }
          throw error
        }
      },
    }),
    {
      name: 'pisper-workspace-order',
      storage,
      // v1 增量增加可选 names，旧数据只含 order 时仍保留顺序并使用默认目录名。
      version: 1,
      partialize: ({ order, names }) => ({ order, names }),
      merge: (persisted, current) => ({
        ...current,
        order: normalizeWorkspaceOrder(
          persisted && typeof persisted === 'object' && 'order' in persisted
            ? persisted.order
            : undefined,
        ),
        names: normalizeWorkspaceNames(
          persisted && typeof persisted === 'object' && 'names' in persisted
            ? persisted.names
            : undefined,
        ),
      }),
    },
  ),
)
