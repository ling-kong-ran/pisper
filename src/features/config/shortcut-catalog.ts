// 快捷键目录（规格 2.8）：“设置 → 快捷键”与 ⌘/ 速查共用同一份分组与文案，保证两处列出的内容一致。
// 可改绑的条目只记动作名，按键一律从 shortcut-store 的当前绑定读取；固定别名（方向键、↵、Esc、⇧↵）
// 不可改绑，只作为只读参考列出。新增动作的文案在按需加载的看板命名空间里，不占入口包体积。
import { useI18n } from '@/app/use-i18n'
import {
  formatShortcut,
  type BoardShortcutAction,
  type BoardShortcutBindings,
  type ShortcutAction,
  type ShortcutBindings,
} from '@/lib/shortcuts'

export type ShortcutGroupId = 'global' | 'sidebar' | 'board' | 'session'
export type FixedShortcutId =
  | 'sidebarMove'
  | 'sidebarOpen'
  | 'boardMove'
  | 'boardSwitchLane'
  | 'boardOpen'
  | 'boardExitTriage'
  | 'newline'
  | 'cycleRunMode'
  | 'stopRun'

export type ShortcutEntry =
  | { kind: 'app'; action: ShortcutAction }
  | { kind: 'board'; action: BoardShortcutAction }
  // keys：可互换的按法，每一种是依次按住的键名。
  | { kind: 'fixed'; id: FixedShortcutId; keys: readonly (readonly string[])[] }

const app = (action: ShortcutAction): ShortcutEntry => ({ kind: 'app', action })
const board = (action: BoardShortcutAction): ShortcutEntry => ({ kind: 'board', action })
const fixed = (id: FixedShortcutId, ...keys: (readonly string[])[]): ShortcutEntry => ({
  kind: 'fixed',
  id,
  keys,
})

// 分组与顺序按规格 v2 第 2.8 节：全局 → 侧栏 → 全部代理 → 会话。v2 侧栏回来后所有动作都列出，没有隐藏动作；
// 默认未绑定的动作（会话主页、组件工作室、侧聊）也列出，方便用户自己绑定。
export const SHORTCUT_GROUPS: readonly {
  id: ShortcutGroupId
  entries: readonly ShortcutEntry[]
}[] = [
  {
    id: 'global',
    entries: [
      app('commandPalette'),
      app('primaryAction'),
      app('toggleSidebar'),
      app('agentPanel'),
      // 侧栏入口：⌘1 全部代理、⌘2 功能、⌘, 设置。
      app('openBoard'),
      app('openMore'),
      app('openSettings'),
      app('jumpToAttention'),
      app('previousAgent'),
      app('nextAgent'),
      app('cycleFocusRegion'),
      app('toggleTerminal'),
      app('shortcutHelp'),
      app('focusComposer'),
      app('voiceInput'),
      app('openSession'),
      app('openComponents'),
    ],
  },
  {
    id: 'sidebar',
    entries: [fixed('sidebarMove', ['↑'], ['↓']), fixed('sidebarOpen', ['Enter'])],
  },
  {
    id: 'board',
    entries: [
      app('focusSearch'),
      board('boardNext'),
      board('boardPrevious'),
      fixed('boardMove', ['↑'], ['↓']),
      board('boardLaneLeft'),
      board('boardLaneRight'),
      fixed('boardSwitchLane', ['←'], ['→']),
      fixed('boardOpen', ['Enter']),
      board('boardApprove'),
      board('boardDeny'),
      board('boardRetry'),
      board('boardTriage'),
      fixed('boardExitTriage', ['Esc']),
      board('boardToggleGrouping'),
    ],
  },
  {
    id: 'session',
    entries: [
      app('toggleInspector'),
      app('sessionTree'),
      app('sideChat'),
      app('sendMessage'),
      app('steerMessage'),
      fixed('newline', ['Shift', 'Enter']),
      // 输入框里 ⇧Tab 在 Plan / Goal / Team 之间切换（规格 v2 第 2.8 节），固定键，不可改绑。
      fixed('cycleRunMode', ['Shift', 'Tab']),
      fixed('stopRun', ['Esc']),
    ],
  },
]

export function shortcutEntryKey(entry: ShortcutEntry) {
  return entry.kind === 'fixed' ? `fixed:${entry.id}` : entry.action
}

// 当前按键：可改绑的条目读绑定（未绑定返回 null），固定别名原样返回。
export function shortcutEntryKeys(
  entry: ShortcutEntry,
  bindings: ShortcutBindings,
  boardBindings: BoardShortcutBindings,
): readonly (readonly string[])[] | null {
  if (entry.kind === 'fixed') return entry.keys
  const label = formatShortcut(
    entry.kind === 'app' ? bindings[entry.action] : boardBindings[entry.action],
  )
  return label ? [label.split(' + ')] : null
}

// 文案全部用字面量键，供 i18n 静态检查。
export function useShortcutCatalogText() {
  const { t } = useI18n()
  const actions: Record<ShortcutAction, string> = {
    commandPalette: t('config:shortcuts.commandPalette'),
    primaryAction: t('config:shortcuts.primaryAction'),
    focusSearch: t('board:shortcuts.focusSearch'),
    toggleSidebar: t('config:shortcuts.toggleSidebar'),
    toggleTerminal: t('config:shortcuts.toggleTerminal'),
    openSettings: t('config:shortcuts.openSettings'),
    focusComposer: t('config:shortcuts.focusComposer'),
    sendMessage: t('board:shortcuts.sendMessage'),
    voiceInput: t('config:shortcuts.voiceInput'),
    openBoard: t('config:shortcuts.openBoard'),
    openComponents: t('config:shortcuts.openComponents'),
    openMore: t('config:shortcuts.openMore'),
    jumpToAttention: t('config:shortcuts.jumpToAttention'),
    agentPanel: t('board:shortcuts.agentPanel'),
    toggleInspector: t('board:shortcuts.toggleInspector'),
    previousAgent: t('board:shortcuts.previousAgent'),
    nextAgent: t('board:shortcuts.nextAgent'),
    steerMessage: t('board:shortcuts.steerMessage'),
    shortcutHelp: t('board:shortcuts.shortcutHelp'),
    openSession: t('board:shortcuts.openSession'),
    sessionTree: t('board:shortcuts.sessionTree'),
    sideChat: t('board:shortcuts.sideChat'),
    cycleFocusRegion: t('board:shortcuts.cycleFocusRegion'),
  }
  const boardActions: Record<BoardShortcutAction, string> = {
    boardNext: t('config:shortcuts.boardNext'),
    boardPrevious: t('config:shortcuts.boardPrevious'),
    boardLaneLeft: t('config:shortcuts.boardLaneLeft'),
    boardLaneRight: t('config:shortcuts.boardLaneRight'),
    boardApprove: t('config:shortcuts.boardApprove'),
    boardDeny: t('config:shortcuts.boardDeny'),
    boardRetry: t('config:shortcuts.boardRetry'),
    boardTriage: t('config:shortcuts.boardTriage'),
    boardToggleGrouping: t('config:shortcuts.boardToggleGrouping'),
  }
  const fixedEntries: Record<FixedShortcutId, string> = {
    sidebarMove: t('board:shortcuts.sidebarMove'),
    sidebarOpen: t('board:shortcuts.sidebarOpen'),
    boardMove: t('board:shortcuts.boardMove'),
    boardSwitchLane: t('board:shortcuts.boardSwitchLane'),
    boardOpen: t('board:shortcuts.boardOpen'),
    boardExitTriage: t('board:shortcuts.boardExitTriage'),
    newline: t('board:shortcuts.newline'),
    cycleRunMode: t('board:shortcuts.cycleRunMode'),
    stopRun: t('board:shortcuts.stopRun'),
  }
  const groups: Record<ShortcutGroupId, { title: string; hint: string }> = {
    global: { title: t('board:shortcuts.groupGlobal'), hint: t('board:shortcuts.globalHint') },
    sidebar: { title: t('board:shortcuts.groupSidebar'), hint: t('board:shortcuts.sidebarHint') },
    board: { title: t('board:shortcuts.groupBoard'), hint: t('board:shortcuts.boardHint') },
    session: { title: t('board:shortcuts.groupSession'), hint: t('board:shortcuts.sessionHint') },
  }
  const label = (entry: ShortcutEntry) =>
    entry.kind === 'app'
      ? actions[entry.action]
      : entry.kind === 'board'
        ? boardActions[entry.action]
        : fixedEntries[entry.id]
  return { actions, boardActions, groups, label }
}
