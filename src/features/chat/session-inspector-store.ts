// 会话页检查器的布局偏好（规格 v2 第 4.4 节）：开关、当前标签、宽度与“固定检查器”，跨会话共用并经偏好同步保留。
// 每个会话里各标签的阅读位置、终端资源与预览地址仍在 session-context-pages-store（内存）。
// v2 起检查器默认关着、按需滑入；“固定”（pinned，默认关）打开后像第 4 期一样常驻，开关状态跨页面加载保留。
// pinned 是只做加法的新字段：旧版本读到它会忽略，新版本读旧值时当作未固定。
// @public 设置 › 界面读写“固定检查器”；模块只有偏好状态，不导入组件。
import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'
import { pageStateStorage } from '@/lib/page-state-storage'

// 'running' 已移到会话顶栏的芯片，不再是标签；保留取值只为读旧偏好时不报错（检查器把它退回默认标签）。
export const SESSION_INSPECTOR_TABS = [
  'changes',
  'terminal',
  'running',
  'files',
  'preview',
  'plan',
  'side-chat',
  // 只读查看某个子代理的过程：打开过子代理时才出现；旧版本读到这个值会退回默认标签。
  'subagent',
] as const
export type SessionInspectorTab = (typeof SESSION_INSPECTOR_TABS)[number]
export type SessionInspectorTerminalView = 'terminal' | 'processes'
// 改动标签的两个范围：本会话（/file-changes 快照）与工作区（/vcs/changes）。
export type SessionInspectorChangesView = 'session' | 'workspace'
// 改动详情的差异视图：统一（默认，适合 456px 检查器）或并排。
export type SessionInspectorDiffMode = 'unified' | 'split'

// 宽度按占会话区的比例保存，大屏上跟着变宽。默认值在 1440 宽的窗口里约等于原来的 456px。
export const SESSION_INSPECTOR_DEFAULT_PERCENT = 39
export const SESSION_INSPECTOR_MAX_PERCENT = 80
// 比例的下限只防脏数据，真正的下限是 360px：8K 屏上 360px 也只占不到 5%。
const SESSION_INSPECTOR_MIN_PERCENT = 3
export const SESSION_INSPECTOR_MIN_WIDTH = 360
// 旧版保存的像素宽度。还留着这个字段：旧版本读到偏好时仍能用。
export const SESSION_INSPECTOR_LEGACY_DEFAULT_WIDTH = 456
const SESSION_INSPECTOR_LEGACY_MAX_WIDTH = 720
// ≤1279 宽或 ≤760 高时检查器改为右侧滑出的覆盖层，宽度固定。
export const SESSION_INSPECTOR_OVERLAY_WIDTH = 420
// 对话列至少保留的宽度：拖宽检查器时不把对话挤到不可读。
export const SESSION_CONVERSATION_MIN_WIDTH = 480

type SessionInspectorPreferences = {
  open: boolean
  pinned: boolean
  tab: SessionInspectorTab
  // 占会话区的百分比。null：旧版拖过的像素宽度，换算不出当时的比例，显示保持原样，下次拖动才变成比例。
  widthPercent: number | null
  width: number
  terminalView: SessionInspectorTerminalView
  changesView: SessionInspectorChangesView
  diffMode: SessionInspectorDiffMode
}

type SessionInspectorState = SessionInspectorPreferences & {
  setOpen: (open: boolean) => void
  setPinned: (pinned: boolean) => void
  setTab: (tab: SessionInspectorTab) => void
  // 拖动或键盘调整后的像素宽度，连同当时的会话区宽度一起换算成比例。
  setWidth: (width: number, available: number) => void
  resetWidth: () => void
  setTerminalView: (view: SessionInspectorTerminalView) => void
  setChangesView: (view: SessionInspectorChangesView) => void
  setDiffMode: (mode: SessionInspectorDiffMode) => void
}

export function normalizeInspectorWidth(value: unknown) {
  if (typeof value !== 'number' || !Number.isFinite(value))
    return SESSION_INSPECTOR_LEGACY_DEFAULT_WIDTH
  return Math.min(
    SESSION_INSPECTOR_LEGACY_MAX_WIDTH,
    Math.max(SESSION_INSPECTOR_MIN_WIDTH, Math.round(value)),
  )
}

function clampInspectorPercent(value: number) {
  const percent = Math.min(
    SESSION_INSPECTOR_MAX_PERCENT,
    Math.max(SESSION_INSPECTOR_MIN_PERCENT, value),
  )
  return Math.round(percent * 100) / 100
}

// 没拖过的旧偏好（默认 456）用新的默认比例；拖过的保持原来的像素宽度，升级后不会突然变样。
function normalizeInspectorPercent(percent: unknown, width: unknown) {
  if (typeof percent === 'number' && Number.isFinite(percent)) return clampInspectorPercent(percent)
  return typeof width === 'number' &&
    normalizeInspectorWidth(width) !== SESSION_INSPECTOR_LEGACY_DEFAULT_WIDTH
    ? null
    : SESSION_INSPECTOR_DEFAULT_PERCENT
}

// 临时空间不足只收窄显示，不改写用户的宽度偏好（与旧上下文分栏同一原则）。
export function clampInspectorWidth(width: number, available: number) {
  const maximum = Math.max(
    SESSION_INSPECTOR_MIN_WIDTH,
    Math.min(
      (available * SESSION_INSPECTOR_MAX_PERCENT) / 100,
      available - SESSION_CONVERSATION_MIN_WIDTH - 8,
    ),
  )
  return Math.round(Math.min(maximum, Math.max(SESSION_INSPECTOR_MIN_WIDTH, width)))
}

export function inspectorDisplayWidth(
  { widthPercent, width }: Pick<SessionInspectorPreferences, 'widthPercent' | 'width'>,
  available: number,
) {
  return clampInspectorWidth(
    widthPercent === null ? normalizeInspectorWidth(width) : (available * widthPercent) / 100,
    available,
  )
}

export function normalizeInspectorPreferences(value: unknown): SessionInspectorPreferences {
  const record =
    value && typeof value === 'object' && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : {}
  const tab = SESSION_INSPECTOR_TABS.find((entry) => entry === record.tab) ?? 'changes'
  const pinned = record.pinned === true
  return {
    // 未固定时每次加载都从关闭开始（按需滑入，规格 4.4）；固定后沿用第 4 期的行为：默认展开，收起后记住。
    // 第 4 期留下的 open: true（当时默认展开）因此不会让检查器在 v2 里自己弹出来。
    open: pinned && record.open !== false,
    pinned,
    // 侧聊是临时标签：恢复时回到改动，不在下次打开会话时自动挂起一条侧聊。
    tab: tab === 'side-chat' ? 'changes' : tab,
    widthPercent: normalizeInspectorPercent(record.widthPercent, record.width),
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
      // 打开“固定”时顺带展开（固定的意思是常驻）；取消固定不收起当前已打开的面板。
      setPinned: (pinned) => set(pinned ? { pinned, open: true } : { pinned }),
      setTab: (tab) => {
        if (get().tab !== tab) set({ tab })
      },
      setWidth: (value, available) => {
        if (!(available > 0)) return
        const widthPercent = clampInspectorPercent((value / available) * 100)
        if (get().widthPercent !== widthPercent)
          set({ widthPercent, width: normalizeInspectorWidth(value) })
      },
      resetWidth: () =>
        set({
          widthPercent: SESSION_INSPECTOR_DEFAULT_PERCENT,
          width: SESSION_INSPECTOR_LEGACY_DEFAULT_WIDTH,
        }),
      setTerminalView: (terminalView) => set({ terminalView }),
      setChangesView: (changesView) => set({ changesView }),
      setDiffMode: (diffMode) => set({ diffMode }),
    }),
    {
      name: SESSION_INSPECTOR_STORAGE_KEY,
      version: 1,
      storage,
      partialize: ({
        open,
        pinned,
        tab,
        widthPercent,
        width,
        terminalView,
        changesView,
        diffMode,
      }) => ({
        open,
        pinned,
        tab,
        widthPercent,
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
