// 会话页检查器的布局偏好（规格 4.4）：开关、当前标签与宽度，跨会话共用并经偏好同步保留。
// 每个会话里各标签的阅读位置、终端资源与预览地址仍在 session-context-pages-store（内存）。
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import { pageStateStorage } from '@/lib/page-state-storage'

export const SESSION_INSPECTOR_TABS = [
  'changes',
  'terminal',
  'files',
  'preview',
  'plan',
  'side-chat',
] as const
export type SessionInspectorTab = (typeof SESSION_INSPECTOR_TABS)[number]
export type SessionInspectorTerminalView = 'terminal' | 'processes'
// 改动标签的两个范围：本会话（/file-changes 快照）与工作区（/vcs/changes）。
export type SessionInspectorChangesView = 'session' | 'workspace'
// 改动详情的差异视图：统一（默认，适合 456px 检查器）或并排。
export type SessionInspectorDiffMode = 'unified' | 'split'

export const SESSION_INSPECTOR_DEFAULT_WIDTH = 456
export const SESSION_INSPECTOR_MIN_WIDTH = 360
export const SESSION_INSPECTOR_MAX_WIDTH = 720
// ≤1279 宽或 ≤760 高时检查器改为右侧滑出的覆盖层，宽度固定。
export const SESSION_INSPECTOR_OVERLAY_WIDTH = 420
// 对话列至少保留的宽度：拖宽检查器时不把对话挤到不可读。
export const SESSION_CONVERSATION_MIN_WIDTH = 480

type SessionInspectorPreferences = {
  open: boolean
  tab: SessionInspectorTab
  width: number
  terminalView: SessionInspectorTerminalView
  changesView: SessionInspectorChangesView
  diffMode: SessionInspectorDiffMode
}

type SessionInspectorState = SessionInspectorPreferences & {
  setOpen: (open: boolean) => void
  setTab: (tab: SessionInspectorTab) => void
  setWidth: (width: number) => void
  setTerminalView: (view: SessionInspectorTerminalView) => void
  setChangesView: (view: SessionInspectorChangesView) => void
  setDiffMode: (mode: SessionInspectorDiffMode) => void
}

export function normalizeInspectorWidth(value: unknown) {
  if (typeof value !== 'number' || !Number.isFinite(value)) return SESSION_INSPECTOR_DEFAULT_WIDTH
  return Math.min(
    SESSION_INSPECTOR_MAX_WIDTH,
    Math.max(SESSION_INSPECTOR_MIN_WIDTH, Math.round(value)),
  )
}

// 临时空间不足只收窄显示，不改写用户的宽度偏好（与旧上下文分栏同一原则）。
export function inspectorDisplayWidth(preferred: number, available: number) {
  const maximum = Math.max(
    SESSION_INSPECTOR_MIN_WIDTH,
    Math.min(SESSION_INSPECTOR_MAX_WIDTH, available - SESSION_CONVERSATION_MIN_WIDTH - 8),
  )
  return Math.min(maximum, normalizeInspectorWidth(preferred))
}

export function normalizeInspectorPreferences(value: unknown): SessionInspectorPreferences {
  const record =
    value && typeof value === 'object' && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : {}
  const tab = SESSION_INSPECTOR_TABS.find((entry) => entry === record.tab) ?? 'changes'
  return {
    // 宽屏默认展开（规格 4.1 的两栏布局）；用户收起后记住。紧凑窗口另用覆盖层，默认关闭。
    open: record.open !== false,
    // 侧聊是临时标签：恢复时回到改动，不在下次打开会话时自动挂起一条侧聊。
    tab: tab === 'side-chat' ? 'changes' : tab,
    width: normalizeInspectorWidth(record.width),
    terminalView: record.terminalView === 'processes' ? 'processes' : 'terminal',
    changesView: record.changesView === 'workspace' ? 'workspace' : 'session',
    diffMode: record.diffMode === 'split' ? 'split' : 'unified',
  }
}

// 隐私模式或 WebView 策略可能禁用本地存储；仍保留本次页面中的调整。
const storage = createJSONStorage<SessionInspectorPreferences>(() => ({
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
      // 持久化失败不回滚当前布局。
    }
  },
  removeItem: (name) => {
    try {
      pageStateStorage.removeItem(name)
    } catch {
      // 存储不可用时没有可清理的状态。
    }
  },
}))

export const SESSION_INSPECTOR_STORAGE_KEY = 'pisper-session-inspector'

export const useSessionInspectorStore = create<SessionInspectorState>()(
  persist(
    (set, get) => ({
      ...normalizeInspectorPreferences(undefined),
      setOpen: (open) => {
        if (get().open !== open) set({ open })
      },
      setTab: (tab) => {
        if (get().tab !== tab) set({ tab })
      },
      setWidth: (value) => {
        const width = normalizeInspectorWidth(value)
        if (get().width !== width) set({ width })
      },
      setTerminalView: (terminalView) => set({ terminalView }),
      setChangesView: (changesView) => set({ changesView }),
      setDiffMode: (diffMode) => set({ diffMode }),
    }),
    {
      name: SESSION_INSPECTOR_STORAGE_KEY,
      version: 1,
      storage,
      partialize: ({ open, tab, width, terminalView, changesView, diffMode }) => ({
        open,
        tab,
        width,
        terminalView,
        changesView,
        diffMode,
      }),
      migrate: (persisted) => normalizeInspectorPreferences(persisted),
      merge: (persisted, current) => ({ ...current, ...normalizeInspectorPreferences(persisted) }),
    },
  ),
)
