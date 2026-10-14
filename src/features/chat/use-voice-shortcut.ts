import { useEffect, useRef } from 'react'
import { matchesShortcut } from '@/lib/shortcuts'

type VoiceShortcutOptions = {
  enabled: boolean
  binding: string | null
  onStart: () => boolean
  onRelease: () => void
  onCancel: () => void
}

function blocksVoiceShortcut(event: KeyboardEvent) {
  const target = event.target instanceof Element ? event.target : null
  if (
    target?.closest(
      '[data-shortcut-recorder], [role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], .xterm, .terminal-xterm, .terminal-panel, [data-terminal]',
    )
  )
    return true
  return Array.from(
    document.querySelectorAll('[role="dialog"], [role="alertdialog"], dialog[open]'),
  ).some((dialog) => dialog.getClientRects().length > 0)
}

export function useVoiceShortcut(options: VoiceShortcutOptions) {
  const callbacks = useRef(options)
  callbacks.current = options
  const { enabled, binding } = options

  useEffect(() => {
    if (!enabled || !binding) return undefined
    let pressedCode: string | null = null
    let hold: {
      code: string
      ctrl: boolean
      meta: boolean
      alt: boolean
      shift: boolean
    } | null = null

    const cancel = () => {
      pressedCode = null
      if (!hold) return
      hold = null
      callbacks.current.onCancel()
    }
    const keydown = (event: KeyboardEvent) => {
      if (hold && event.key === 'Escape' && !event.isComposing) {
        cancel()
        event.preventDefault()
        return
      }
      if (
        !callbacks.current.enabled ||
        event.defaultPrevented ||
        event.isComposing ||
        event.keyCode === 229 ||
        event.repeat ||
        pressedCode ||
        document.visibilityState === 'hidden' ||
        blocksVoiceShortcut(event) ||
        !matchesShortcut(event, binding)
      )
        return
      pressedCode = event.code
      if (!callbacks.current.onStart()) return
      hold = {
        code: event.code,
        ctrl: event.ctrlKey,
        meta: event.metaKey,
        alt: event.altKey,
        shift: event.shiftKey,
      }
      event.preventDefault()
    }
    const keyup = (event: KeyboardEvent) => {
      if (event.code === pressedCode) pressedCode = null
      if (!hold) return
      // 松键走捕获阶段且不检查焦点，避免弹层或修饰键乱序释放让麦克风持续工作。
      if (
        event.code === hold.code ||
        (hold.ctrl && (!event.ctrlKey || event.code.startsWith('Control'))) ||
        (hold.meta && (!event.metaKey || event.code.startsWith('Meta'))) ||
        (hold.alt && (!event.altKey || event.code.startsWith('Alt'))) ||
        (hold.shift && (!event.shiftKey || event.code.startsWith('Shift')))
      ) {
        hold = null
        callbacks.current.onRelease()
      }
    }
    const visibility = () => {
      if (document.visibilityState === 'hidden') cancel()
    }
    // 按下在冒泡阶段尊重编辑器和快捷键录制器，松开则始终保证可以结束本轮录音。
    window.addEventListener('keydown', keydown)
    window.addEventListener('keyup', keyup, true)
    window.addEventListener('blur', cancel)
    document.addEventListener('visibilitychange', visibility)
    return () => {
      window.removeEventListener('keydown', keydown)
      window.removeEventListener('keyup', keyup, true)
      window.removeEventListener('blur', cancel)
      document.removeEventListener('visibilitychange', visibility)
      cancel()
    }
  }, [enabled, binding])
}
