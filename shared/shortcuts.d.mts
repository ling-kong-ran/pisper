export const SHORTCUT_ACTIONS: readonly [
  'commandPalette',
  'primaryAction',
  'focusSearch',
  'toggleSidebar',
  'toggleTerminal',
  'openSettings',
  'focusComposer',
  'sendMessage',
  'voiceInput',
]

export type ShortcutAction = (typeof SHORTCUT_ACTIONS)[number]
export type ShortcutBindings = Record<ShortcutAction, string | null>

export type ShortcutKeyboardEvent = {
  code?: string
  key?: string
  ctrlKey?: boolean
  metaKey?: boolean
  altKey?: boolean
  shiftKey?: boolean
  defaultPrevented?: boolean
  repeat?: boolean
  isComposing?: boolean
  keyCode?: number
  getModifierState?: (key: string) => boolean
}

export const DEFAULT_SHORTCUTS: Readonly<ShortcutBindings>

export class ShortcutValidationError extends Error {
  constructor(
    message: string,
    code?: 'invalid' | 'conflict',
    action?: ShortcutAction | null,
    conflictingAction?: ShortcutAction | null,
  )
  code: 'invalid' | 'conflict'
  action: ShortcutAction | null
  conflictingAction: ShortcutAction | null
}

export function validateShortcutBindings(input: unknown): ShortcutBindings
export function normalizeShortcutBindings(input: unknown): ShortcutBindings
export function findShortcutConflict(
  bindings: Partial<ShortcutBindings>,
  action: ShortcutAction,
  candidate: string | null,
): ShortcutAction | null
export function shortcutFromEvent(event: ShortcutKeyboardEvent, mac?: boolean): string | null
export function matchesShortcut(
  event: ShortcutKeyboardEvent,
  binding: string | null,
  mac?: boolean,
): boolean
export function formatShortcut(binding: string | null, mac?: boolean): string
