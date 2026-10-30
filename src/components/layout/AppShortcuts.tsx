import { useEffect } from 'react'
import {
  isEditableTarget,
  matchesShortcut,
  optionTypingInEditable,
  shortcutEventBlocked,
} from '@/lib/shortcuts'
import { useShortcutStore } from '@/stores/shortcut-store'

// 应用级快捷键：带修饰键的组合在输入框内同样生效（⌘K、⌘N、⌘1/2/3/4、⌘,、⌥A、⇧⌘B、⌘/），
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
  onShowBoard,
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
  // 会话页泳道条的“看板”入口（打开看板）；只在会话页与主页传入。
  onShowBoard?: () => void
  onShortcutHelp?: () => void
}) {
  const bindings = useShortcutStore((state) => state.bindings)
  useEffect(() => {
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
      else if (matchesShortcut(event, bindings.showBoard)) action = onShowBoard
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
    onJumpToAttention,
    onOpenBoard,
    onOpenComponents,
    onOpenMore,
    onOpenSession,
    onPrimary,
    onSearch,
    onSettings,
    onShortcutHelp,
    onShowBoard,
    onToggleTerminal,
    onToggleSidebar,
  ])
  return null
}
