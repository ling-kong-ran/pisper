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

export type ShortcutGroupId = 'global' | 'board' | 'session'
export type FixedShortcutId =
  'boardMove' | 'boardSwitchLane' | 'boardOpen' | 'boardExitTriage' | 'newline' | 'stopRun'

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

// 新外壳没有侧栏，“展开 / 收起侧栏”不再有作用；绑定仍保留在存储里，只是不列出。
export const HIDDEN_SHORTCUTS: readonly ShortcutAction[] = ['toggleSidebar']

export const SHORTCUT_GROUPS: readonly {
  id: ShortcutGroupId
  entries: readonly ShortcutEntry[]
}[] = [
  {
    id: 'global',
    entries: [
      app('commandPalette'),
      app('primaryAction'),
      app('openBoard'),
      app('openComponents'),
      app('openMore'),
      app('openSettings'),
      app('jumpToAttention'),
      app('shortcutHelp'),
      app('toggleTerminal'),
    ],
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
      app('showBoard'),
      app('backToBoard'),
      app('previousAgent'),
      app('nextAgent'),
      app('agentPanel'),
      app('toggleInspector'),
      app('sendMessage'),
      app('steerMessage'),
      fixed('newline', ['Shift', 'Enter']),
      fixed('stopRun', ['Esc']),
      app('focusComposer'),
      app('voiceInput'),
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
    backToBoard: t('config:shortcuts.backToBoard'),
    agentPanel: t('board:shortcuts.agentPanel'),
    toggleInspector: t('board:shortcuts.toggleInspector'),
    previousAgent: t('board:shortcuts.previousAgent'),
    nextAgent: t('board:shortcuts.nextAgent'),
    steerMessage: t('board:shortcuts.steerMessage'),
    shortcutHelp: t('board:shortcuts.shortcutHelp'),
    showBoard: t('board:shortcuts.showBoard'),
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
    boardMove: t('board:shortcuts.boardMove'),
    boardSwitchLane: t('board:shortcuts.boardSwitchLane'),
    boardOpen: t('board:shortcuts.boardOpen'),
    boardExitTriage: t('board:shortcuts.boardExitTriage'),
    newline: t('board:shortcuts.newline'),
    stopRun: t('board:shortcuts.stopRun'),
  }
  const groups: Record<ShortcutGroupId, { title: string; hint: string }> = {
    global: { title: t('board:shortcuts.groupGlobal'), hint: t('board:shortcuts.globalHint') },
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
