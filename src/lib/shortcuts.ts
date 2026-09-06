import {
  formatShortcut as formatBinding,
  matchesShortcut as matchesBinding,
  shortcutFromEvent as recordBinding,
  type ShortcutAction,
  type ShortcutKeyboardEvent,
} from '@shared/shortcuts.mjs'
import { useShortcutStore } from '@/stores/shortcut-store'

export * from '@shared/shortcuts.mjs'

const usesCommandKey = /Mac|iPhone|iPad/.test(globalThis.navigator?.platform || '')

export function formatShortcut(binding: string | null, mac = usesCommandKey) {
  return formatBinding(binding, mac)
}

export function matchesShortcut(
  event: ShortcutKeyboardEvent,
  binding: string | null,
  mac = usesCommandKey,
) {
  return matchesBinding(event, binding, mac)
}

export function shortcutFromEvent(event: ShortcutKeyboardEvent, mac = usesCommandKey) {
  return recordBinding(event, mac)
}

export function useShortcutLabel(action: ShortcutAction) {
  return formatShortcut(useShortcutStore((state) => state.bindings[action]))
}

export function isEditableTarget(target: EventTarget | null) {
  return (
    target instanceof HTMLElement &&
    Boolean(
      target.closest(
        'input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="textbox"]',
      ),
    )
  )
}

export function shortcutEventBlocked(event: KeyboardEvent) {
  return (
    event.defaultPrevented ||
    event.isComposing ||
    event.keyCode === 229 ||
    event.repeat ||
    document.visibilityState === 'hidden' ||
    Boolean(document.querySelector('[role="dialog"], [role="alertdialog"], dialog[open]')) ||
    (event.target instanceof HTMLElement &&
      Boolean(
        event.target.closest(
          '[data-shortcut-recorder], .terminal-panel, [role="menu"], [role="listbox"]',
        ),
      ))
  )
}
