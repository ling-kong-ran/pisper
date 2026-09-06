export const SHORTCUT_ACTIONS = Object.freeze([
  'commandPalette',
  'primaryAction',
  'focusSearch',
  'toggleSidebar',
  'toggleTerminal',
  'openSettings',
  'focusComposer',
  'sendMessage',
  'voiceInput',
])

export const DEFAULT_SHORTCUTS = Object.freeze({
  commandPalette: 'Mod+KeyK',
  primaryAction: 'Mod+KeyN',
  focusSearch: 'Slash',
  toggleSidebar: 'Mod+KeyB',
  toggleTerminal: 'Mod+Backquote',
  openSettings: 'Mod+Comma',
  focusComposer: 'Mod+Shift+KeyL',
  sendMessage: 'Enter',
  voiceInput: 'F8',
})

const MODIFIERS = ['Mod', 'Ctrl', 'Meta', 'Alt', 'Shift']
const CHARACTER_CODES = new Set([
  'Space',
  'Backquote',
  'Minus',
  'Equal',
  'BracketLeft',
  'BracketRight',
  'Backslash',
  'Semicolon',
  'Quote',
  'Comma',
  'Period',
  'Slash',
  'IntlBackslash',
  'IntlRo',
  'IntlYen',
  'NumpadAdd',
  'NumpadSubtract',
  'NumpadMultiply',
  'NumpadDivide',
  'NumpadDecimal',
])
const RESERVED_PRIMARY_CODES = new Set([
  'KeyA',
  'KeyC',
  'KeyF',
  'KeyL',
  'KeyO',
  'KeyP',
  'KeyQ',
  'KeyR',
  'KeyS',
  'KeyT',
  'KeyV',
  'KeyW',
  'KeyX',
  'KeyY',
  'KeyZ',
])
const KEY_CODES = new Map([
  [' ', 'Space'],
  ['Spacebar', 'Space'],
  ['`', 'Backquote'],
  ['~', 'Backquote'],
  ['-', 'Minus'],
  ['_', 'Minus'],
  ['=', 'Equal'],
  ['+', 'Equal'],
  ['[', 'BracketLeft'],
  ['{', 'BracketLeft'],
  [']', 'BracketRight'],
  ['}', 'BracketRight'],
  ['\\', 'Backslash'],
  ['|', 'Backslash'],
  [';', 'Semicolon'],
  [':', 'Semicolon'],
  ["'", 'Quote'],
  ['"', 'Quote'],
  [',', 'Comma'],
  ['<', 'Comma'],
  ['.', 'Period'],
  ['>', 'Period'],
  ['/', 'Slash'],
  ['?', 'Slash'],
  ...[...')!@#$%^&*('].map((key, index) => [key, `Digit${index}`]),
])

export class ShortcutValidationError extends Error {
  constructor(message, code = 'invalid', action = null, conflictingAction = null) {
    super(message)
    this.name = 'ShortcutValidationError'
    this.code = code
    this.action = action
    this.conflictingAction = conflictingAction
  }
}

function parseShortcut(binding) {
  if (typeof binding !== 'string' || !binding) return null
  const parts = binding.split('+')
  const code = parts.pop()
  if (
    !/^(Key[A-Z]|Digit[0-9]|Numpad[0-9]|F([1-9]|1[0-2])|Enter)$/.test(code) &&
    !CHARACTER_CODES.has(code)
  ) {
    return null
  }
  let previous = -1
  for (const modifier of parts) {
    const index = MODIFIERS.indexOf(modifier)
    if (index <= previous) return null
    previous = index
  }
  if (parts.filter((part) => ['Mod', 'Ctrl', 'Meta'].includes(part)).length > 1) {
    return null
  }
  return {
    code,
    primary: parts.find((part) => ['Mod', 'Ctrl', 'Meta'].includes(part)) || null,
    alt: parts.includes('Alt'),
    shift: parts.includes('Shift'),
  }
}

function isAllowed(shortcut, action = null) {
  const { code, primary, alt, shift } = shortcut
  if (code === 'Enter' && shift && !primary && !alt) return false
  if (alt && (code === 'F4' || code === 'Space')) return false
  if (primary && code === 'F4') return false
  if ((primary === 'Mod' || primary === 'Meta') && code === 'Space' && !shift) {
    return false
  }
  // 默认聚焦输入框使用此组合，其余系统编辑和浏览器导航组合不能被覆盖。
  if (primary && RESERVED_PRIMARY_CODES.has(code)) {
    if (!(code === 'KeyL' && shift)) return false
  }
  if (/^F\d+$/.test(code)) return true
  if (primary || alt) return true
  if (shift) return false
  return (
    (code === 'Enter' && (!action || action === 'sendMessage')) ||
    (code === 'Slash' && (!action || action === 'focusSearch'))
  )
}

function modifierState(shortcut, mac) {
  return {
    ctrl: shortcut.primary === 'Ctrl' || (shortcut.primary === 'Mod' && !mac),
    meta: shortcut.primary === 'Meta' || (shortcut.primary === 'Mod' && mac),
    alt: shortcut.alt,
    shift: shortcut.shift,
  }
}

function sameShortcut(left, right, mac) {
  const a = modifierState(left, mac)
  const b = modifierState(right, mac)
  return (
    left.code === right.code &&
    a.ctrl === b.ctrl &&
    a.meta === b.meta &&
    a.alt === b.alt &&
    a.shift === b.shift
  )
}

export function findShortcutConflict(bindings, action, candidate) {
  const shortcut = parseShortcut(candidate)
  if (!shortcut) return null
  for (const otherAction of SHORTCUT_ACTIONS) {
    if (otherAction === action) continue
    const other = parseShortcut(bindings?.[otherAction])
    if (other && (sameShortcut(shortcut, other, false) || sameShortcut(shortcut, other, true))) {
      return otherAction
    }
  }
  return null
}

export function validateShortcutBindings(input) {
  if (
    !input ||
    typeof input !== 'object' ||
    Array.isArray(input) ||
    ![Object.prototype, null].includes(Object.getPrototypeOf(input)) ||
    Reflect.ownKeys(input).length !== SHORTCUT_ACTIONS.length ||
    !SHORTCUT_ACTIONS.every((action) => Object.hasOwn(input, action))
  ) {
    throw new ShortcutValidationError('Expected a complete shortcut map with only known actions')
  }
  const bindings = {}
  for (const action of SHORTCUT_ACTIONS) {
    const binding = input[action]
    if (binding !== null) {
      const shortcut = parseShortcut(binding)
      if (!shortcut || !isAllowed(shortcut, action)) {
        throw new ShortcutValidationError(
          `Invalid or reserved shortcut for ${action}`,
          'invalid',
          action,
        )
      }
    }
    bindings[action] = binding
  }
  for (const action of SHORTCUT_ACTIONS) {
    const other = findShortcutConflict(bindings, action, bindings[action])
    if (other) {
      throw new ShortcutValidationError(
        `Shortcut conflict between ${action} and ${other}`,
        'conflict',
        action,
        other,
      )
    }
  }
  return bindings
}

export function normalizeShortcutBindings(input) {
  try {
    const bindings = validateShortcutBindings(input)
    // 升级旧默认键时保留自定义和禁用设置，已占用 F8 的操作不能被覆盖。
    if (
      bindings.voiceInput === 'Mod+Shift+Space' &&
      !findShortcutConflict(bindings, 'voiceInput', 'F8')
    ) {
      bindings.voiceInput = 'F8'
    }
    return bindings
  } catch {
    return { ...DEFAULT_SHORTCUTS }
  }
}

function eventCode(event) {
  if (event.code) return event.code
  const key = event.key
  if (typeof key !== 'string') return null
  if (/^[a-z]$/i.test(key)) return `Key${key.toUpperCase()}`
  if (/^[0-9]$/.test(key)) return `Digit${key}`
  if (key === 'Enter' || /^F([1-9]|1[0-2])$/.test(key)) return key
  return KEY_CODES.get(key) || null
}

function ignoredEvent(event) {
  return (
    !event ||
    event.defaultPrevented ||
    event.repeat ||
    event.isComposing ||
    event.keyCode === 229 ||
    ['Dead', 'Process', 'Unidentified', 'AltGraph'].includes(event.key) ||
    event.getModifierState?.('AltGraph')
  )
}

export function shortcutFromEvent(event, mac = false) {
  if (ignoredEvent(event)) return null
  if (event.ctrlKey && event.metaKey) return null
  const code = eventCode(event)
  if (!code) return null
  const parts = []
  if (mac ? event.metaKey : event.ctrlKey) parts.push('Mod')
  else if (event.ctrlKey) parts.push('Ctrl')
  else if (event.metaKey) parts.push('Meta')
  if (event.altKey) parts.push('Alt')
  if (event.shiftKey) parts.push('Shift')
  parts.push(code)
  const binding = parts.join('+')
  const shortcut = parseShortcut(binding)
  return shortcut && isAllowed(shortcut) ? binding : null
}

export function matchesShortcut(event, binding, mac = false) {
  if (ignoredEvent(event)) return false
  const shortcut = parseShortcut(binding)
  if (!shortcut || !isAllowed(shortcut) || eventCode(event) !== shortcut.code) return false
  const modifiers = modifierState(shortcut, mac)
  return (
    Boolean(event.ctrlKey) === modifiers.ctrl &&
    Boolean(event.metaKey) === modifiers.meta &&
    Boolean(event.altKey) === modifiers.alt &&
    Boolean(event.shiftKey) === modifiers.shift
  )
}

export function formatShortcut(binding, mac = false) {
  const shortcut = parseShortcut(binding)
  if (!shortcut) return ''
  const parts = []
  if (shortcut.primary) {
    parts.push(
      shortcut.primary === 'Mod'
        ? mac
          ? 'Cmd'
          : 'Ctrl'
        : shortcut.primary === 'Meta'
          ? 'Cmd'
          : 'Ctrl',
    )
  }
  if (shortcut.alt) parts.push('Alt')
  if (shortcut.shift) parts.push('Shift')
  const labels = {
    Backquote: '`',
    Minus: '-',
    Equal: '=',
    BracketLeft: '[',
    BracketRight: ']',
    Backslash: '\\',
    Semicolon: ';',
    Quote: "'",
    Comma: ',',
    Period: '.',
    Slash: '/',
    IntlBackslash: '\\',
    IntlRo: '\\',
    IntlYen: 'Yen',
    NumpadAdd: 'Num +',
    NumpadSubtract: 'Num -',
    NumpadMultiply: 'Num *',
    NumpadDivide: 'Num /',
    NumpadDecimal: 'Num .',
  }
  const code = shortcut.code
  parts.push(labels[code] || code.replace(/^Key|^Digit/, '').replace(/^Numpad/, 'Num '))
  return parts.join(' + ')
}
