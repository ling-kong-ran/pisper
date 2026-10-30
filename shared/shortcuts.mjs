export const SHORTCUT_ACTIONS = Object.freeze([
  'commandPalette',
  'primaryAction',
  'focusSearch',
  // 方向 A（规格 v2 第 2.8 节）：侧栏回来了，⌘B 收起 / 展开侧栏重新生效（第 1–4 期是隐藏动作）。
  'toggleSidebar',
  'toggleTerminal',
  'openSettings',
  'focusComposer',
  'sendMessage',
  'voiceInput',
  // 一级入口：⌘1 全部代理（openBoard，页面 id 仍是 board）、⌘2 功能（openMore）；
  // 组件工作室不再是一级入口，openComponents 保留为可改绑、默认未绑定的动作。
  'openBoard',
  'openComponents',
  'openMore',
  // ⌥A：聚焦侧栏里等待最久的需要你行（侧栏收起时临时展开），再按 A / D 处理普通批准、↵ 打开。
  'jumpToAttention',
  // ⌘J：聚焦侧栏会话列表（原为代理面板覆盖层，v2 改义）。
  'agentPanel',
  'toggleInspector',
  // ⌥↑ / ⌥↓：按侧栏顺序切换到上一个 / 下一个会话。
  'previousAgent',
  'nextAgent',
  'steerMessage',
  'shortcutHelp',
  // 主页“会话”入口：v2 没有一级标签了，保留为可改绑、默认未绑定的动作。
  // showBoard（泳道条的看板入口）在 v2 退役：看板没了，它占的 ⇧⌘B 交给会话树。
  'openSession',
  // v2 新动作追加在末尾，旧动作的顺序不变。
  'sessionTree',
  'sideChat',
  'cycleFocusRegion',
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
  openBoard: 'Mod+Digit1',
  openComponents: null,
  openMore: 'Mod+Digit2',
  jumpToAttention: 'Alt+KeyA',
  agentPanel: 'Mod+KeyJ',
  toggleInspector: 'Mod+KeyI',
  previousAgent: 'Alt+ArrowUp',
  nextAgent: 'Alt+ArrowDown',
  steerMessage: 'Alt+Enter',
  shortcutHelp: 'Mod+Slash',
  openSession: null,
  // 会话树（分支 + 标记合并后的唯一入口）接手退役的 showBoard 的 ⇧⌘B。
  sessionTree: 'Mod+Shift+KeyB',
  // 侧聊默认不绑定：样稿的 ⌘⇧L 已是“聚焦聊天输入框”。
  sideChat: null,
  // F6 在侧栏、顶栏、对话、输入框之间切换焦点区域（桌面应用的惯例键）。
  cycleFocusRegion: 'F6',
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
    !/^(Key[A-Z]|Digit[0-9]|Numpad[0-9]|F([1-9]|1[0-2])|Enter|Arrow(Up|Down))$/.test(code) &&
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
  // 方向键只开放 ⌥↑ / ⌥↓：加 ⌘/Ctrl 或 Shift 是文本跳转与选择，⌥←/→ 是浏览器前进后退。
  if (code.startsWith('Arrow')) return alt && !primary && !shift
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

// 第 4 期标签重新编号之前的默认键（看板 ⌘1、组件 ⌘2、更多 ⌘3）。
const LEGACY_TAB_SHORTCUTS = Object.freeze({
  openBoard: 'Mod+Digit1',
  openComponents: 'Mod+Digit2',
  openMore: 'Mod+Digit3',
})
// 83140e4d 的标签默认键：⌘1 会话、⌘2 看板、⌘3 组件、⌘4 更多。写死在这里而不是读 DEFAULT_SHORTCUTS，
// 因为 v2 改了默认键，链式迁移的中间一步必须落在当时的布局上。
const TAB_SHORTCUTS_83140E4D = Object.freeze({
  openSession: 'Mod+Digit1',
  openBoard: 'Mod+Digit2',
  openComponents: 'Mod+Digit3',
  openMore: 'Mod+Digit4',
})
// 后移顺序：从 ⌘4 往回，前一个让出的键才轮到后一个；最后“会话”接手 ⌘1。
const RENUMBERED_TABS = Object.freeze(['openMore', 'openComponents', 'openBoard', 'openSession'])
// 方向 A 的入口键：⌘1 全部代理、⌘2 功能，会话与组件不再有默认键。
const SIDEBAR_ENTRY_SHORTCUTS = Object.freeze({
  openSession: null,
  openBoard: 'Mod+Digit1',
  openComponents: null,
  openMore: 'Mod+Digit2',
})

// 没有 openSession 的表来自标签重新编号之前的版本。三个标签键都还是旧默认（用户没动过）时整体后移一位，
// 让没改过键的人拿到与标签顺序一致的布局；每一步只移到没被其他动作占用的键上，移不动就保持原样，
// 后面的随之留在原处。改过其中任何一个就一律不动：自定义优先于新布局。
function renumberLegacyTabs(bindings) {
  for (const [action, binding] of Object.entries(LEGACY_TAB_SHORTCUTS)) {
    if (bindings[action] !== binding) return
  }
  for (const action of RENUMBERED_TABS) {
    const target = TAB_SHORTCUTS_83140E4D[action]
    if (!findShortcutConflict(bindings, action, target)) bindings[action] = target
  }
}

// 没有 sessionTree 的表来自方向 A 之前的版本（规格 v2 第 2.8 节）。四个入口键正好是 83140e4d 的默认
// （⌘1 会话 / ⌘2 看板 / ⌘3 组件 / ⌘4 更多）时整体换成新入口键；改过其中任何一个（含禁用、平台别名）
// 就原样保留。新键只在这四个动作之间腾挪（⌘1、⌘2 本来就属于它们），不会抢走其他动作的键。
function adoptSidebarEntries(bindings) {
  for (const [action, binding] of Object.entries(TAB_SHORTCUTS_83140E4D)) {
    if (bindings[action] !== binding) return
  }
  Object.assign(bindings, SIDEBAR_ENTRY_SHORTCUTS)
}

// 旧版本保存的绑定表缺少后来新增的动作：缺失项补默认键，但默认键已被用户占用时补为未绑定，
// 已有的自定义与禁用设置原样保留；未知动作（含已退役的 backToBoard、showBoard）直接丢弃，它占过的键随之空出，
// 所以 sessionTree 只在 ⇧⌘B 空着或仍是 showBoard 的默认时拿到它。只有补齐后仍不合法才整体回到默认。
// 表里已有 sessionTree 时不再迁移入口键，所以重复归一化结果不变。
function withAddedActions(input) {
  if (
    !input ||
    typeof input !== 'object' ||
    Array.isArray(input) ||
    ![Object.prototype, null].includes(Object.getPrototypeOf(input))
  ) {
    return input
  }
  const known = SHORTCUT_ACTIONS.filter((action) => Object.hasOwn(input, action))
  if (known.length === SHORTCUT_ACTIONS.length && Reflect.ownKeys(input).length === known.length) {
    return input
  }
  const bindings = {}
  for (const action of known) bindings[action] = input[action]
  if (!Object.hasOwn(bindings, 'sessionTree')) {
    if (!Object.hasOwn(bindings, 'openSession')) renumberLegacyTabs(bindings)
    adoptSidebarEntries(bindings)
  }
  for (const action of SHORTCUT_ACTIONS) {
    if (Object.hasOwn(bindings, action)) continue
    const fallback = DEFAULT_SHORTCUTS[action]
    bindings[action] = findShortcutConflict(bindings, action, fallback) ? null : fallback
  }
  return bindings
}

export function normalizeShortcutBindings(input) {
  try {
    const bindings = validateShortcutBindings(withAddedActions(input))
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
  if (/^(Enter|Arrow(Up|Down)|F([1-9]|1[0-2]))$/.test(key)) return key
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
    ArrowUp: '↑',
    ArrowDown: '↓',
  }
  const code = shortcut.code
  parts.push(labels[code] || code.replace(/^Key|^Digit/, '').replace(/^Numpad/, 'Num '))
  return parts.join(' + ')
}

// —— 看板单字母快捷键 ——
// 看板上的 J/K/H/L、A/D/R、T、G 是单字母键：只在看板页且焦点不在输入框时生效，
// 因此与应用级组合键分开登记、分开校验，不放宽上面“裸字母键保留给打字”的规则。
// 只接受不带修饰键的 KeyA–KeyZ；方向键、↵、Esc 是固定别名，不可改绑。
export const BOARD_SHORTCUT_ACTIONS = Object.freeze([
  'boardNext',
  'boardPrevious',
  'boardLaneLeft',
  'boardLaneRight',
  'boardApprove',
  'boardDeny',
  'boardRetry',
  'boardTriage',
  'boardToggleGrouping',
])

export const DEFAULT_BOARD_SHORTCUTS = Object.freeze({
  boardNext: 'KeyJ',
  boardPrevious: 'KeyK',
  boardLaneLeft: 'KeyH',
  boardLaneRight: 'KeyL',
  boardApprove: 'KeyA',
  boardDeny: 'KeyD',
  boardRetry: 'KeyR',
  boardTriage: 'KeyT',
  boardToggleGrouping: 'KeyG',
})

const BOARD_KEY_PATTERN = /^Key[A-Z]$/

export function findBoardShortcutConflict(bindings, action, candidate) {
  if (typeof candidate !== 'string' || !BOARD_KEY_PATTERN.test(candidate)) return null
  return (
    BOARD_SHORTCUT_ACTIONS.find(
      (otherAction) => otherAction !== action && bindings?.[otherAction] === candidate,
    ) || null
  )
}

export function validateBoardShortcutBindings(input) {
  if (
    !input ||
    typeof input !== 'object' ||
    Array.isArray(input) ||
    ![Object.prototype, null].includes(Object.getPrototypeOf(input)) ||
    Reflect.ownKeys(input).length !== BOARD_SHORTCUT_ACTIONS.length ||
    !BOARD_SHORTCUT_ACTIONS.every((action) => Object.hasOwn(input, action))
  ) {
    throw new ShortcutValidationError('Expected a complete board shortcut map')
  }
  const bindings = {}
  for (const action of BOARD_SHORTCUT_ACTIONS) {
    const binding = input[action]
    if (binding !== null && (typeof binding !== 'string' || !BOARD_KEY_PATTERN.test(binding))) {
      throw new ShortcutValidationError(`Invalid board shortcut for ${action}`, 'invalid', action)
    }
    bindings[action] = binding
  }
  for (const action of BOARD_SHORTCUT_ACTIONS) {
    const other = findBoardShortcutConflict(bindings, action, bindings[action])
    if (other) {
      throw new ShortcutValidationError(
        `Board shortcut conflict between ${action} and ${other}`,
        'conflict',
        action,
        other,
      )
    }
  }
  return bindings
}

// 与应用级绑定同样容忍旧数据：缺失项补默认键（默认键已被占用则补为未绑定），未知项丢弃。
export function normalizeBoardShortcutBindings(input) {
  try {
    const source =
      input && typeof input === 'object' && !Array.isArray(input) ? input : Object.create(null)
    const bindings = {}
    for (const action of BOARD_SHORTCUT_ACTIONS) {
      if (Object.hasOwn(source, action)) bindings[action] = source[action]
    }
    for (const action of BOARD_SHORTCUT_ACTIONS) {
      if (Object.hasOwn(bindings, action)) continue
      const fallback = DEFAULT_BOARD_SHORTCUTS[action]
      bindings[action] = findBoardShortcutConflict(bindings, action, fallback) ? null : fallback
    }
    return validateBoardShortcutBindings(bindings)
  } catch {
    return { ...DEFAULT_BOARD_SHORTCUTS }
  }
}

function hasModifier(event) {
  return Boolean(event?.ctrlKey || event?.metaKey || event?.altKey || event?.shiftKey)
}

export function boardShortcutFromEvent(event) {
  if (ignoredEvent(event) || hasModifier(event)) return null
  const code = eventCode(event)
  return code && BOARD_KEY_PATTERN.test(code) ? code : null
}

export function matchesBoardShortcut(event, binding) {
  if (typeof binding !== 'string' || !BOARD_KEY_PATTERN.test(binding)) return false
  return boardShortcutFromEvent(event) === binding
}
