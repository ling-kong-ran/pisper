// 设置 → 快捷键（规格 2.8）：按“全局 / 看板 / 会话”分组列出全部绑定，分组与 ⌘/ 速查共用
// shortcut-catalog。可改绑的条目走录制控件；方向键、Enter、Esc、Shift + Enter 等固定别名只读列出。
import { useState, type KeyboardEvent } from 'react'
import { Ellipsis, RotateCcw, X } from 'lucide-react'
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
import {
  HIDDEN_SHORTCUTS,
  SHORTCUT_GROUPS,
  shortcutEntryKey,
  useShortcutCatalogText,
  type ShortcutEntry,
} from './shortcut-catalog'
import { ShortcutKeys } from './ShortcutKeys'

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
  const text = useShortcutCatalogText()
  const labels = text.actions
  const boardLabels = text.boardActions
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

  // 固定别名：只读参考，与可改绑行对齐到同一列，标明“固定”。
  const renderEntry = (entry: ShortcutEntry) => {
    if (entry.kind !== 'fixed') return renderShortcut(entry.action)
    return (
      <div
        key={shortcutEntryKey(entry)}
        className="flex min-w-0 flex-wrap items-center justify-between gap-x-5 gap-y-2 py-2.5"
        data-shortcut-fixed={entry.id}
      >
        <span className="min-w-0 text-sm text-content-secondary">
          {text.label(entry)}
          <span className="ml-1.5 text-xs text-content-muted">{t('board:shortcuts.fixed')}</span>
        </span>
        <div className="flex max-w-full min-w-0 items-center gap-1 max-[650px]:w-full">
          <ShortcutKeys
            keys={entry.keys}
            className="h-8.5 w-44 justify-center px-2.5 max-[650px]:flex-1"
          />
          {/* 占住“⋯”菜单那一列，键帽与可改绑行的录制框对齐。 */}
          <span
            aria-hidden="true"
            className="size-7 flex-none pointer-coarse:size-11 max-[650px]:size-11"
          />
        </div>
      </div>
    )
  }

  return (
    <section className="w-full min-w-0" data-config-card="shortcuts-bindings">
      {/* 页标题已经是“快捷键”，这里不再重复卡片标题，只留“全部恢复默认”。 */}
      <div className="mb-1 flex items-center justify-end gap-3">
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
      <div className="flex flex-col gap-5">
        {SHORTCUT_GROUPS.map((group) => (
          <section
            key={group.id}
            aria-labelledby={`shortcut-group-${group.id}`}
            data-shortcut-group={group.id}
          >
            <h3
              id={`shortcut-group-${group.id}`}
              className="text-[13px] leading-6 font-semibold text-content"
            >
              {text.groups[group.id].title}
            </h3>
            <p className="mb-2 text-xs text-content-muted">{text.groups[group.id].hint}</p>
            <SettingsGroup>{group.entries.map(renderEntry)}</SettingsGroup>
          </section>
        ))}
      </div>
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
