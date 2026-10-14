import { useEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { attachComponentBridge } from './component-bridge'
import { startCustomUiView } from './custom-ui-view'
import { customUiComponentLabel } from './custom-ui-labels'
import type { CustomUiComponent, CustomUiView } from './custom-ui-api'

export function CustomUiFrame({
  component,
  notify,
  preview = false,
}: {
  component: CustomUiComponent
  notify?: (message: string) => void
  preview?: boolean
}) {
  const { t, locale } = useI18n()
  const frameRef = useRef<HTMLIFrameElement>(null)
  const stopRef = useRef<(() => void) | null>(null)
  const notifyRef = useRef(notify)
  notifyRef.current = notify
  const [failed, setFailed] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const [view, setView] = useState<CustomUiView | null>(null)

  useEffect(() => {
    setFailed(false)
    setView(null)
    const stop = startCustomUiView(component.id, setView, () => setFailed(true))
    stopRef.current = stop
    return () => {
      stop()
      stopRef.current = null
    }
  }, [component.id, component.entry, attempt])

  useEffect(() => {
    const iframe = frameRef.current
    if (!iframe || failed || !view) return
    return attachComponentBridge(iframe, {
      component,
      preview,
      locale,
      notify: (message) => notifyRef.current?.(message),
    })
  }, [component, preview, view, failed, locale])

  if (failed) {
    return (
      <div
        role="status"
        className="flex h-full min-h-0 min-w-0 flex-col items-center justify-center gap-2 overflow-auto p-3 text-center text-xs text-[var(--text-muted)]"
      >
        <p>{t('custom-ui:customUiPage.componentFailedToLoad')}</p>
        <Button variant="outline" size="sm" onClick={() => setAttempt((value) => value + 1)}>
          {t('custom-ui:widget.retry')}
        </Button>
      </div>
    )
  }
  if (!view) {
    return (
      <div
        role="status"
        className="grid h-full min-h-0 min-w-0 place-items-center p-3 text-xs text-[var(--text-muted)]"
      >
        {t('custom-ui:customUiPage.loading')}
      </div>
    )
  }
  return (
    <iframe
      ref={frameRef}
      title={customUiComponentLabel(component, t)}
      src={view.entryUrl}
      referrerPolicy="no-referrer"
      // 不加 allow-same-origin：组件处于 opaque origin，只能经桥与应用交互。
      sandbox="allow-scripts"
      inert={preview || undefined}
      tabIndex={preview ? -1 : undefined}
      className={cn(
        'block h-full min-h-0 w-full min-w-0 border-0 bg-transparent',
        preview && 'pointer-events-none',
      )}
      onError={() => {
        stopRef.current?.()
        setFailed(true)
      }}
    />
  )
}
