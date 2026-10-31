// 侧栏偏好（规格 v2 第 2.1 节）：收起 / 展开状态的唯一所有者，存在 'pisper-sidebar'，
// 经 page-state-storage 落盘并同步到 Runtime 镜像（随机回环端口换了也不丢）。
// 第 1–4 期 ui-store 里的 sidebarCollapsed / sidebarWidth 属于早已不挂载的旧侧栏，已经移除：
// 侧栏宽度固定 268（不可拖动），旧的收起值也不迁移——它记录的是旧侧栏，而 v2 要求侧栏默认展开占位，
// 只有用户在新侧栏里亲手收起才变窄轨。
import { create } from 'zustand'
import { STORAGE_KEYS } from '@/app/storage'
import { pageStateStorage } from '@/lib/page-state-storage'

export type SidebarPreference = {
  // 用户收起成 60px 窄轨。
  collapsed: boolean
  // “今天完成”分组折叠（规格 2.1：今天完成可折叠；需要你永不折叠）。
  doneCollapsed: boolean
}

export const DEFAULT_SIDEBAR_PREFERENCE: Readonly<SidebarPreference> = Object.freeze({
  collapsed: false,
  doneCollapsed: false,
})

// 存储内容按不可信输入解析：坏 JSON、错类型、未知字段一律回到默认值，永不抛错。
export function parseSidebarPreference(raw: unknown): SidebarPreference {
  let value: unknown = raw
  if (typeof raw === 'string') {
    try {
      value = JSON.parse(raw)
    } catch {
      return { ...DEFAULT_SIDEBAR_PREFERENCE }
    }
  }
  if (!value || typeof value !== 'object' || Array.isArray(value))
    return { ...DEFAULT_SIDEBAR_PREFERENCE }
  const source = value as Record<string, unknown>
  return {
    collapsed: source.collapsed === true,
    doneCollapsed: source.doneCollapsed === true,
  }
}

export function serializeSidebarPreference(preference: SidebarPreference) {
  return JSON.stringify({
    collapsed: preference.collapsed === true,
    doneCollapsed: preference.doneCollapsed === true,
  })
}

function readPreference(): SidebarPreference {
  try {
    return parseSidebarPreference(pageStateStorage.getItem(STORAGE_KEYS.sidebar))
  } catch {
    return { ...DEFAULT_SIDEBAR_PREFERENCE }
  }
}

type SidebarState = SidebarPreference & {
  // 收起时 ⌘J / ⌥A 临时展开侧栏（规格 2.8）：只在内存里，不落盘，焦点离开侧栏后由外壳收回。
  peeking: boolean
  // 手机外壳的抽屉（规格 7.1）：内容与桌面侧栏相同，只用 ☰ 打开；只在内存里，不落盘。
  drawerOpen: boolean
  setCollapsed: (collapsed: boolean) => void
  toggleCollapsed: () => void
  setDoneCollapsed: (collapsed: boolean) => void
  setPeeking: (peeking: boolean) => void
  setDrawerOpen: (open: boolean) => void
}

export const useSidebarStore = create<SidebarState>((set, get) => {
  const save = (patch: Partial<SidebarPreference>) => {
    const next = { ...get(), ...patch }
    // 存储不可用（隐私模式、配额）时界面照常切换，只是这次选择不会保留到下次启动。
    try {
      pageStateStorage.setItem(STORAGE_KEYS.sidebar, serializeSidebarPreference(next))
    } catch {
      // 忽略：侧栏状态不是关键数据。
    }
    set(patch)
  }
  return {
    ...readPreference(),
    peeking: false,
    drawerOpen: false,
    // 亲手收起或展开都结束临时展开，避免“收起”按钮按下去侧栏却还开着。
    setCollapsed: (collapsed) => {
      set({ peeking: false })
      save({ collapsed })
    },
    toggleCollapsed: () => {
      set({ peeking: false })
      save({ collapsed: !get().collapsed })
    },
    setDoneCollapsed: (doneCollapsed) => save({ doneCollapsed }),
    setPeeking: (peeking) => set({ peeking }),
    setDrawerOpen: (drawerOpen) => set({ drawerOpen }),
  }
})

// 其他标签页改了侧栏偏好时跟着变（与 shortcut-store 一致）；只在浏览器环境挂监听。
if (typeof window !== 'undefined') {
  window.addEventListener('storage', (event) => {
    if (event.key === STORAGE_KEYS.sidebar || event.key === null)
      useSidebarStore.setState(readPreference())
  })
}
