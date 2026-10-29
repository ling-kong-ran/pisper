import { useState, type KeyboardEvent } from 'react'
import { ChevronDown, Ellipsis, Keyboard, RotateCcw, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Kbd, KbdGroup } from '@/components/ui/kbd'
import { cn } from '@/lib/utils'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useShortcutStore } from '@/stores/shortcut-store'
import {
  BOARD_SHORTCUT_ACTIONS,
  DEFAULT_BOARD_SHORTCUTS,
  DEFAULT_SHORTCUTS,
  SHORTCUT_ACTIONS,
  boardShortcutFromEvent,
  findBoardShortcutConflict,
  findShortcutConflict,
  formatShortcut,
  shortcutFromEvent,
  validateShortcutBindings,
  type BoardShortcutAction,
  type ShortcutAction,
} from '@/lib/shortcuts'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { SettingsGroup } from './settings-primitives'

const COMMON_SHORTCUTS: ShortcutAction[] = [
  'voiceInput',
  'sendMessage',
  'commandPalette',
  'primaryAction',
  'openBoard',
  'jumpToAttention',
]
// 新外壳没有侧栏，“展开 / 收起侧栏”不再有作用；绑定仍保留在存储里，只是不在设置里列出。
const HIDDEN_SHORTCUTS: ShortcutAction[] = ['toggleSidebar']
const MORE_SHORTCUTS = SHORTCUT_ACTIONS.filter(
  (action) => !COMMON_SHORTCUTS.includes(action) && !HIDDEN_SHORTCUTS.includes(action),
)

export function ShortcutSettings({
  requestConfirm,
}: {
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t } = useI18n()
  const bindings = useShortcutStore((state) => state.bindings)
  const boardBindings = useShortcutStore((state) => state.boardBindings)
  const setBinding = useShortcutStore((state) => state.setBinding)
  const setBoardBinding = useShortcutStore((state) => state.setBoardBinding)
  const resetBindings = useShortcutStore((state) => state.resetBindings)
  const [recording, setRecording] = useState<ShortcutAction | BoardShortcutAction | null>(null)
  const [error, setError] = useState<{
    action: ShortcutAction | BoardShortcutAction | null
    message: string
  } | null>(null)
  const [saved, setSaved] = useState(false)
  const labels: Record<ShortcutAction, string> = {
    commandPalette: t('config:shortcuts.commandPalette'),
    primaryAction: t('config:shortcuts.primaryAction'),
    focusSearch: t('config:shortcuts.focusSearch'),
    toggleSidebar: t('config:shortcuts.toggleSidebar'),
    toggleTerminal: t('config:shortcuts.toggleTerminal'),
    openSettings: t('config:shortcuts.openSettings'),
    focusComposer: t('config:shortcuts.focusComposer'),
    sendMessage: t('config:shortcuts.sendMessage'),
    voiceInput: t('config:shortcuts.voiceInput'),
    openBoard: t('config:shortcuts.openBoard'),
    openComponents: t('config:shortcuts.openComponents'),
    openMore: t('config:shortcuts.openMore'),
    jumpToAttention: t('config:shortcuts.jumpToAttention'),
    backToBoard: t('config:shortcuts.backToBoard'),
  }
  const boardLabels: Record<BoardShortcutAction, string> = {
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
  // 看板单字母键：只接受不带修饰键的字母，同表内不能重复。
  const changeBoard = (action: BoardShortcutAction, binding: string | null) => {
    setSaved(false)
    const conflict = findBoardShortcutConflict(boardBindings, action, binding)
    if (conflict) {
      setError({
        action,
        message: t('config:shortcuts.conflict', { action: boardLabels[conflict] }),
      })
      return
    }
    try {
      setBoardBinding(action, binding)
      setRecording(null)
      setError(null)
      setSaved(true)
    } catch {
      setError({ action, message: t('config:shortcuts.saveFailed') })
    }
  }
  const change = (action: ShortcutAction, binding: string | null) => {
    setSaved(false)
    const conflict = findShortcutConflict(bindings, action, binding)
    // 设置里隐藏的动作（侧栏开关：新外壳没有侧栏）不能挡住可见动作，否则会提示与一个看不见的动作冲突；
    // 直接让出它的按键，存储形状不变。
    const hiddenConflict = conflict && HIDDEN_SHORTCUTS.includes(conflict) ? conflict : null
    if (conflict && !hiddenConflict) {
      setError({ action, message: t('config:shortcuts.conflict', { action: labels[conflict] }) })
      return
    }
    try {
      validateShortcutBindings({
        ...bindings,
        ...(hiddenConflict ? { [hiddenConflict]: null } : {}),
        [action]: binding,
      })
    } catch {
      setError({ action, message: t('config:shortcuts.invalid') })
      return
    }
    try {
      if (hiddenConflict) setBinding(hiddenConflict, null)
      setBinding(action, binding)
      setRecording(null)
      setError(null)
      setSaved(true)
    } catch {
      setError({ action, message: t('config:shortcuts.saveFailed') })
    }
  }
  const recordKey = (
    event: KeyboardEvent<HTMLButtonElement>,
    action: ShortcutAction | BoardShortcutAction,
  ) => {
    if (recording !== action) return
    if (event.key === 'Tab') {
      setRecording(null)
      return
    }
    const board = (BOARD_SHORTCUT_ACTIONS as readonly string[]).includes(action)
    const binding = board
      ? boardShortcutFromEvent(event.nativeEvent)
      : shortcutFromEvent(event.nativeEvent)
    event.preventDefault()
    event.stopPropagation()
    if (event.key === 'Escape') {
      setRecording(null)
      setError(null)
      return
    }
    if (binding && board) changeBoard(action as BoardShortcutAction, binding)
    else if (binding) change(action as ShortcutAction, binding)
    else if (
      !event.nativeEvent.isComposing &&
      event.keyCode !== 229 &&
      !event.repeat &&
      !['Control', 'Meta', 'Alt', 'Shift', 'AltGraph', 'Dead', 'Process'].includes(event.key)
    ) {
      setError({ action, message: t('config:shortcuts.invalid') })
    }
  }
  const reset = async () => {
    setRecording(null)
    const confirmed = await requestConfirm({
      title: t('config:shortcuts.resetAll'),
      message: t('config:shortcuts.resetConfirm'),
      confirmLabel: t('config:shortcuts.resetAll'),
    })
    if (!confirmed) return
    try {
      resetBindings()
      setError(null)
      setSaved(true)
    } catch {
      setError({ action: null, message: t('config:shortcuts.saveFailed') })
    }
  }
  const changed =
    SHORTCUT_ACTIONS.some((action) => bindings[action] !== DEFAULT_SHORTCUTS[action]) ||
    BOARD_SHORTCUT_ACTIONS.some(
      (action) => boardBindings[action] !== DEFAULT_BOARD_SHORTCUTS[action],
    )

  const renderShortcut = (action: ShortcutAction | BoardShortcutAction) => {
    const board = (BOARD_SHORTCUT_ACTIONS as readonly string[]).includes(action)
    const label = board
      ? boardLabels[action as BoardShortcutAction]
      : labels[action as ShortcutAction]
    const binding = board
      ? boardBindings[action as BoardShortcutAction]
      : bindings[action as ShortcutAction]
    const fallback = board
      ? DEFAULT_BOARD_SHORTCUTS[action as BoardShortcutAction]
      : DEFAULT_SHORTCUTS[action as ShortcutAction]
    const apply = (next: string | null) =>
      board
        ? changeBoard(action as BoardShortcutAction, next)
        : change(action as ShortcutAction, next)
    return (
      <div key={action} className="py-2.5" data-shortcut-action={action}>
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-5 gap-y-2">
          <label
            htmlFor={`shortcut-${action}`}
            className="min-w-0 text-sm font-medium text-content"
          >
            {label}
          </label>
          <div className="flex max-w-full min-w-0 items-center gap-1 max-[650px]:w-full">
            {/* 录制框画成字段：按键读数用小键帽逐键呈现；录制中换成强调色焦点边框。 */}
            <Button
              id={`shortcut-${action}`}
              data-shortcut-recorder
              variant="outline"
              className={cn(
                'h-8.5 w-44 min-w-0 justify-center gap-2 border-input bg-panel px-2.5 text-xs font-normal shadow-none hover:bg-surface-subtle pointer-coarse:h-11 max-[650px]:h-11 max-[650px]:flex-1',
                recording === action &&
                  'border-focus text-accent-fg ring-3 ring-focus-ring hover:bg-panel',
              )}
              aria-label={t('config:shortcuts.edit', { action: label })}
              aria-pressed={recording === action}
              aria-describedby={error?.action === action ? `shortcut-error-${action}` : undefined}
              onClick={() => {
                setRecording(action)
                setError(null)
                setSaved(false)
              }}
              onBlur={() => setRecording((current) => (current === action ? null : current))}
              onKeyDown={(event) => recordKey(event, action)}
            >
              {recording === action ? (
                <span className="min-w-0 truncate">{t('config:shortcuts.recording')}</span>
              ) : formatShortcut(binding) ? (
                <KbdGroup className="min-w-0 flex-wrap justify-center">
                  {formatShortcut(binding)
                    .split(' + ')
                    .map((key, index) => (
                      <Kbd key={index} className="text-content-secondary">
                        {key}
                      </Kbd>
                    ))}
                </KbdGroup>
              ) : (
                <span className="text-content-muted">{t('config:shortcuts.disabled')}</span>
              )}
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="pointer-coarse:size-11 max-[650px]:size-11"
                  title={t('config:shortcuts.options', { action: label })}
                  aria-label={t('config:shortcuts.options', { action: label })}
                >
                  <Ellipsis className="size-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                align="end"
                className="w-40"
                onCloseAutoFocus={(event) => {
                  // 菜单退场期间已经开始改键时，不再把焦点抢回菜单按钮。
                  if (document.activeElement?.hasAttribute('data-shortcut-recorder'))
                    event.preventDefault()
                }}
              >
                <DropdownMenuItem disabled={binding === null} onSelect={() => apply(null)}>
                  <X className="size-4" />
                  {t('config:shortcuts.disableKey')}
                </DropdownMenuItem>
                <DropdownMenuItem disabled={binding === fallback} onSelect={() => apply(fallback)}>
                  <RotateCcw className="size-4" />
                  {t('config:shortcuts.resetAll')}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
        {error?.action === action && (
          <p id={`shortcut-error-${action}`} role="alert" className="mt-2 text-xs text-danger">
            {error.message}
          </p>
        )}
      </div>
    )
  }

  return (
    <section className="w-full min-w-0" data-config-card="shortcuts-bindings">
      <div className="mb-3 flex items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Keyboard className="size-4 text-content-muted" aria-hidden="true" />
          <h2 className="text-[15px] leading-6 font-medium text-content">
            {t('config:configPage.shortcuts')}
          </h2>
        </div>
        <Button
          variant="ghost"
          size="icon"
          className="pointer-coarse:size-11 max-[650px]:size-11"
          disabled={!changed}
          title={t('config:shortcuts.resetAll')}
          aria-label={t('config:shortcuts.resetAll')}
          onClick={() => void reset()}
        >
          <RotateCcw className="size-4" />
        </Button>
      </div>
      <SettingsGroup>{COMMON_SHORTCUTS.map(renderShortcut)}</SettingsGroup>
      <details className="group mt-3" onToggle={() => setRecording(null)}>
        <summary className="flex min-h-9 w-fit cursor-pointer list-none items-center gap-1.5 rounded-sm text-sm text-content-secondary hover:text-content max-[650px]:min-h-11 [&::-webkit-details-marker]:hidden">
          <ChevronDown className="size-4 -rotate-90 text-content-muted transition-transform group-open:rotate-0" />
          {t('config:shortcuts.more')}
        </summary>
        <SettingsGroup className="mt-2">{MORE_SHORTCUTS.map(renderShortcut)}</SettingsGroup>
      </details>
      <details className="group/board mt-1" onToggle={() => setRecording(null)}>
        <summary className="flex min-h-9 w-fit cursor-pointer list-none items-center gap-1.5 rounded-sm text-sm text-content-secondary hover:text-content max-[650px]:min-h-11 [&::-webkit-details-marker]:hidden">
          <ChevronDown className="size-4 -rotate-90 text-content-muted transition-transform group-open/board:rotate-0" />
          {t('config:shortcuts.boardGroup')}
        </summary>
        <p className="mt-1 text-xs text-content-muted">{t('config:shortcuts.boardGroupHint')}</p>
        <SettingsGroup className="mt-2">{BOARD_SHORTCUT_ACTIONS.map(renderShortcut)}</SettingsGroup>
      </details>
      <div className="mt-3 min-h-5 text-xs text-content-muted" role="status" aria-live="polite">
        {error?.action === null ? (
          <span className="text-destructive">{error.message}</span>
        ) : saved ? (
          t('config:shortcuts.saved')
        ) : (
          t('config:shortcuts.deviceScope')
        )}
      </div>
    </section>
  )
}
