import { useEffect } from 'react'
import { createPortal } from 'react-dom'
import { Minus, Square, X } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { useDesktopTitlebarHost } from '@/components/layout/DesktopTitlebar'
import { cn } from '@/lib/utils'

export function DesktopWindowControls() {
  const { t } = useI18n()
  const labels = {
    minimize: t('navigation:window.minimize'),
    maximize: t('navigation:window.maximize'),
    close: t('navigation:window.close'),
  }
  const bridge = window.pisperDesktop
  const host = useDesktopTitlebarHost(['window-controls-aside', 'window-controls-chat'])
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
  // 窗口按钮占据标签栏的实际布局空间；侧栏优先接管，关闭后回到主栏工具旁。
  const controls = (
    <div
      data-window-drag-region
      className={cn(
        'desktop-window-controls flex shrink-0 items-stretch justify-end [-webkit-app-region:drag]',
        host ? 'h-full' : 'h-9 border-b border-border/60 bg-background',
      )}
      aria-label={t('navigation:window.controls')}
      onPointerDown={(event) => event.stopPropagation()}
    >
      {(['minimize', 'maximize', 'close'] as const).map((action) => {
        const Icon = action === 'minimize' ? Minus : action === 'maximize' ? Square : X
        return (
          <button
            type="button"
            key={action}
            title={labels[action]}
            aria-label={labels[action]}
            className={`grid w-11 shrink-0 place-items-center text-muted-foreground [-webkit-app-region:no-drag] ${action === 'close' ? 'hover:bg-red-600 hover:text-white' : 'hover:bg-foreground/10'}`}
            onClick={() => void bridge.windowAction?.(action).catch(() => {})}
          >
            <Icon size={action === 'maximize' ? 12 : 15} />
          </button>
        )
      })}
    </div>
  )
  return host ? createPortal(controls, host) : controls
}
