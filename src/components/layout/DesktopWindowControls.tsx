import { useEffect } from 'react'
import { Minus, Square, X } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'

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
  // 窗口按钮独占标题行，主栏与侧栏的标签操作都留在其下方，避免互相遮挡。
  return (
    <div
      data-window-drag-region
      className="desktop-window-controls flex h-9 shrink-0 items-stretch justify-end border-b border-border/60 bg-background [-webkit-app-region:drag]"
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
            className={`grid w-11 place-items-center text-muted-foreground [-webkit-app-region:no-drag] ${action === 'close' ? 'hover:bg-red-600 hover:text-white' : 'hover:bg-foreground/10'}`}
            onClick={() => void bridge.windowAction?.(action).catch(() => {})}
          >
            <Icon size={action === 'maximize' ? 12 : 15} />
          </button>
        )
      })}
    </div>
  )
}
