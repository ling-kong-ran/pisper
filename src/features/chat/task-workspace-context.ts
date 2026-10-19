import { createContext } from 'react'

// 工作区持有可见区域，FocusSession 仍持有编辑器/附件生命周期；插槽不会复制草稿。
export const TaskWorkspaceSlots = createContext<{
  headerHost: HTMLDivElement | null
  sourceActionHost: HTMLDivElement | null
  setSourceActionHost: (element: HTMLDivElement | null) => void
} | null>(null)
