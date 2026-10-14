import { useState, type KeyboardEvent } from 'react'
import { ChevronDown, Ellipsis, Keyboard, RotateCcw, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useShortcutStore } from '@/stores/shortcut-store'
import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_ACTIONS,
  findShortcutConflict,
  formatShortcut,
  shortcutFromEvent,
  validateShortcutBindings,
  type ShortcutAction,
} from '@/lib/shortcuts'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

const COMMON_SHORTCUTS: ShortcutAction[] = [
  'voiceInput',
  'sendMessage',
  'commandPalette',
  'primaryAction',
]
const MORE_SHORTCUTS = SHORTCUT_ACTIONS.filter((action) => !COMMON_SHORTCUTS.includes(action))

export function ShortcutSettings({
  requestConfirm,
}: {
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t } = useI18n()
  const bindings = useShortcutStore((state) => state.bindings)
  const setBinding = useShortcutStore((state) => state.setBinding)
  const resetBindings = useShortcutStore((state) => state.resetBindings)
  const [recording, setRecording] = useState<ShortcutAction | null>(null)
  const [error, setError] = useState<{ action: ShortcutAction | null; message: string } | null>(
    null,
  )
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
  }
  const change = (action: ShortcutAction, binding: string | null) => {
    setSaved(false)
    const conflict = findShortcutConflict(bindings, action, binding)
    if (conflict) {
      setError({ action, message: t('config:shortcuts.conflict', { action: labels[conflict] }) })
      return
    }
    try {
      validateShortcutBindings({ ...bindings, [action]: binding })
    } catch {
      setError({ action, message: t('config:shortcuts.invalid') })
      return
    }
    try {
      setBinding(action, binding)
      setRecording(null)
      setError(null)
      setSaved(true)
    } catch {
      setError({ action, message: t('config:shortcuts.saveFailed') })
    }
  }
  const recordKey = (event: KeyboardEvent<HTMLButtonElement>, action: ShortcutAction) => {
    if (recording !== action) return
    if (event.key === 'Tab') {
      setRecording(null)
      return
    }
    const binding = shortcutFromEvent(event.nativeEvent)
    event.preventDefault()
    event.stopPropagation()
    if (event.key === 'Escape') {
      setRecording(null)
      setError(null)
      return
    }
    if (binding) change(action, binding)
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
  const changed = SHORTCUT_ACTIONS.some((action) => bindings[action] !== DEFAULT_SHORTCUTS[action])

  const renderShortcut = (action: ShortcutAction) => (
    <div key={action} className="py-3" data-shortcut-action={action}>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-5 gap-y-2">
        <label htmlFor={`shortcut-${action}`} className="min-w-0 text-sm font-medium">
          {labels[action]}
        </label>
        <div className="flex min-w-0 max-w-full items-center gap-1">
          <Button
            id={`shortcut-${action}`}
            data-shortcut-recorder
            variant="outline"
            className="h-9 w-44 min-w-0 justify-center gap-2 px-2.5 text-xs max-[400px]:w-40"
            aria-label={t('config:shortcuts.edit', { action: labels[action] })}
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
            <span className="min-w-0 whitespace-normal break-words text-left">
              {recording === action
                ? t('config:shortcuts.recording')
                : formatShortcut(bindings[action]) || t('config:shortcuts.disabled')}
            </span>
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                title={t('config:shortcuts.options', { action: labels[action] })}
                aria-label={t('config:shortcuts.options', { action: labels[action] })}
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
              <DropdownMenuItem
                disabled={bindings[action] === null}
                onSelect={() => change(action, null)}
              >
                <X className="size-4" />
                {t('config:shortcuts.disableKey')}
              </DropdownMenuItem>
              <DropdownMenuItem
                disabled={bindings[action] === DEFAULT_SHORTCUTS[action]}
                onSelect={() => change(action, DEFAULT_SHORTCUTS[action])}
              >
                <RotateCcw className="size-4" />
                {t('config:shortcuts.resetAll')}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      {error?.action === action && (
        <p id={`shortcut-error-${action}`} role="alert" className="mt-2 text-sm text-destructive">
          {error.message}
        </p>
      )}
    </div>
  )

  return (
    <section className="w-full min-w-0" data-config-card="shortcuts-bindings">
      <div className="mb-5 flex items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Keyboard className="size-5 text-muted-foreground" aria-hidden="true" />
          <h2 className="text-base font-semibold">{t('config:configPage.shortcuts')}</h2>
        </div>
        <Button
          variant="ghost"
          size="icon"
          disabled={!changed}
          title={t('config:shortcuts.resetAll')}
          aria-label={t('config:shortcuts.resetAll')}
          onClick={() => void reset()}
        >
          <RotateCcw className="size-4" />
        </Button>
      </div>
      <div className="divide-y divide-border border-y border-border">
        {COMMON_SHORTCUTS.map(renderShortcut)}
      </div>
      <details className="group mt-3" onToggle={() => setRecording(null)}>
        <summary className="flex w-fit cursor-pointer list-none items-center gap-1.5 py-2 text-sm text-muted-foreground hover:text-foreground [&::-webkit-details-marker]:hidden">
          <ChevronDown className="size-4 transition-transform group-open:rotate-180" />
          {t('config:shortcuts.more')}
        </summary>
        <div className="divide-y divide-border border-b border-border">
          {MORE_SHORTCUTS.map(renderShortcut)}
        </div>
      </details>
      <div className="mt-3 min-h-5 text-xs text-muted-foreground" role="status" aria-live="polite">
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
