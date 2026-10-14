import { useEffect } from 'react'
import { isEditableTarget, matchesShortcut, shortcutEventBlocked } from '@/lib/shortcuts'
import { useShortcutStore } from '@/stores/shortcut-store'

export function AppShortcuts({
  blocked,
  onCommandPalette,
  onPrimary,
  onSearch,
  onToggleTerminal,
  onToggleSidebar,
  onSettings,
}: {
  blocked: boolean
  onCommandPalette: () => void
  onPrimary: () => void
  onSearch: () => void
  onToggleTerminal?: () => void
  onToggleSidebar: () => void
  onSettings: () => void
}) {
  const bindings = useShortcutStore((state) => state.bindings)
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (blocked || shortcutEventBlocked(event)) return
      let action: (() => void) | undefined
      if (matchesShortcut(event, bindings.commandPalette)) action = onCommandPalette
      else if (matchesShortcut(event, bindings.toggleSidebar)) action = onToggleSidebar
      else if (matchesShortcut(event, bindings.toggleTerminal)) action = onToggleTerminal
      else if (matchesShortcut(event, bindings.openSettings)) action = onSettings
      else if (!isEditableTarget(event.target)) {
        if (matchesShortcut(event, bindings.primaryAction)) action = onPrimary
        else if (matchesShortcut(event, bindings.focusSearch)) action = onSearch
      }
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
    onPrimary,
    onSearch,
    onSettings,
    onToggleTerminal,
    onToggleSidebar,
  ])
  return null
}
