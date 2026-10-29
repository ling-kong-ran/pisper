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

// macOS 的 Option（Alt）是输入修饰键：⌥A 打出“å”，⌥E 是重音死键。焦点在可编辑元素内时，
// 只带 Alt（不带 ⌘ / Ctrl）的组合按打字处理，交还输入框，不触发应用级快捷键。
export function optionTypingInEditable(
  event: Pick<KeyboardEvent, 'altKey' | 'ctrlKey' | 'metaKey' | 'target'>,
  mac = usesCommandKey,
) {
  return mac && event.altKey && !event.ctrlKey && !event.metaKey && isEditableTarget(event.target)
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
