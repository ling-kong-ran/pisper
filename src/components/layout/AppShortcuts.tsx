import { useLayoutEffect } from 'react'
import {
  isEditableTarget,
  matchesShortcut,
  optionTypingInEditable,
  shortcutEventBlocked,
} from '@/lib/shortcuts'
import { useShortcutStore } from '@/stores/shortcut-store'

// 应用级快捷键：带修饰键的组合在输入框内同样生效（⌘K、⌘N、⌘B、⌘1/2、⌘,、⌥A、⇧⌘B、⌘/、F6），
// 单键（如 /）只在焦点不在输入框时生效，避免吞掉正常打字；macOS 上输入框内只带 ⌥ 的组合
// 是在输入特殊字符（⌥A → å），同样交还输入框。处理器缺省时不消费按键，让浏览器或页面自己的处理继续。
export function AppShortcuts({
  blocked,
  onCommandPalette,
  onPrimary,
  onSearch,
  onToggleTerminal,
  onToggleSidebar,
  onSettings,
  onOpenSession,
  onOpenBoard,
  onOpenComponents,
  onOpenMore,
  onJumpToAttention,
  onSessionTree,
  onSideChat,
  onCycleFocusRegion,
  onShortcutHelp,
}: {
  blocked: boolean
  onCommandPalette: () => void
  onPrimary: () => void
  onSearch: () => void
  onToggleTerminal?: () => void
  onToggleSidebar?: () => void
  onSettings: () => void
  // 一级标签“会话”：与点标签相同的主页逻辑（上次停留的会话或新任务草稿，从不建会话）。
  onOpenSession?: () => void
  onOpenBoard?: () => void
  onOpenComponents?: () => void
  onOpenMore?: () => void
  onJumpToAttention?: () => void
  // 规格 v2 第 2.8 节的新动作：会话树（⇧⌘B）、侧聊（默认未绑定）只在会话页传入；
  // F6 区域切换由外壳传入。缺省时不消费按键（看板入口动作退役后 ⇧⌘B 不再打开看板）。
  onSessionTree?: () => void
  onSideChat?: () => void
  onCycleFocusRegion?: () => void
  onShortcutHelp?: () => void
}) {
  const bindings = useShortcutStore((state) => state.bindings)
  // 在提交阶段（浏览器绘制之前）挂监听：外壳第一次画出来时快捷键就已可用，冷启动不丢键。
  useLayoutEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (blocked || shortcutEventBlocked(event) || optionTypingInEditable(event)) return
      let action: (() => void) | undefined
      if (matchesShortcut(event, bindings.commandPalette)) action = onCommandPalette
      else if (matchesShortcut(event, bindings.primaryAction)) action = onPrimary
      else if (matchesShortcut(event, bindings.openSession)) action = onOpenSession
      else if (matchesShortcut(event, bindings.openBoard)) action = onOpenBoard
      else if (matchesShortcut(event, bindings.openComponents)) action = onOpenComponents
      else if (matchesShortcut(event, bindings.openMore)) action = onOpenMore
      else if (matchesShortcut(event, bindings.jumpToAttention)) action = onJumpToAttention
      else if (matchesShortcut(event, bindings.sessionTree)) action = onSessionTree
      else if (matchesShortcut(event, bindings.sideChat)) action = onSideChat
      else if (matchesShortcut(event, bindings.cycleFocusRegion)) action = onCycleFocusRegion
      else if (matchesShortcut(event, bindings.toggleSidebar)) action = onToggleSidebar
      else if (matchesShortcut(event, bindings.toggleTerminal)) action = onToggleTerminal
      else if (matchesShortcut(event, bindings.openSettings)) action = onSettings
      else if (matchesShortcut(event, bindings.shortcutHelp)) action = onShortcutHelp
      else if (!isEditableTarget(event.target) && matchesShortcut(event, bindings.focusSearch))
        action = onSearch
      if (!action) return
      event.preventDefault()
      action()
    }
    window.addEventListener('keydown', keydown)
    return () => window.removeEventListener('keydown', keydown)
  }, [
    bindings,
    blocked,
    onCommandPalette,
    onCycleFocusRegion,
    onJumpToAttention,
    onOpenBoard,
    onOpenComponents,
    onOpenMore,
    onOpenSession,
    onPrimary,
    onSearch,
    onSessionTree,
    onSettings,
    onShortcutHelp,
    onSideChat,
    onToggleTerminal,
    onToggleSidebar,
  ])
  return null
}
