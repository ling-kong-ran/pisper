import { useEffect } from 'react'
import { Minus, Square, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'

export function DesktopWindowControls() {
  const { t } = useI18n()
  const labels = {
    minimize: t('navigation:window.minimize'),
    maximize: t('navigation:window.maximize'),
    close: t('navigation:window.close'),
  }
  const bridge = window.pisperDesktop
  useEffect(() => {
    if (!bridge?.customTitlebar || !bridge.windowAction) return
    const interactive =
      'button, a, input, select, textarea, [role="button"], [role="tab"], [contenteditable="true"]'
    const action = (event: MouseEvent) => {
      const target = event.target instanceof Element ? event.target : null
      if (
        event.button !== 0 ||
        !target?.closest('[data-window-drag-region]') ||
        target.closest(interactive)
      )
        return
      if (event.type === 'mousedown' && event.detail > 1) return
      void bridge.windowAction?.(event.type === 'dblclick' ? 'maximize' : 'drag').catch(() => {})
    }
    document.addEventListener('mousedown', action)
    document.addEventListener('dblclick', action)
    return () => {
      document.removeEventListener('mousedown', action)
      document.removeEventListener('dblclick', action)
    }
  }, [bridge])
  if (!bridge?.customTitlebar || !bridge.windowAction) return null
  return (
    <div
      className="desktop-window-controls absolute right-0 top-0 z-40 flex h-12"
      aria-label={t('navigation:window.controls')}
    >
      {(['minimize', 'maximize', 'close'] as const).map((action) => {
        const Icon = action === 'minimize' ? Minus : action === 'maximize' ? Square : X
        return (
          <button
            type="button"
            key={action}
            title={labels[action]}
            aria-label={labels[action]}
            className={`grid w-11 place-items-center text-muted-foreground ${action === 'close' ? 'hover:bg-red-600 hover:text-white' : 'hover:bg-foreground/10'}`}
            onClick={() => void bridge.windowAction?.(action).catch(() => {})}
          >
            <Icon size={action === 'maximize' ? 12 : 15} />
          </button>
        )
      })}
    </div>
  )
}
