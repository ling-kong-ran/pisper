// 手机一次只显示一屏，默认停在对话；Pad 横屏同时放下左中右，不再占用底栏高度。
import { useEffect, useRef, type ReactNode } from 'react'
import { useI18n } from '@/app/i18n/use-i18n'
import { cn } from '@/lib/utils'
import {
  mobileShellPaneIndex,
  moveMobileShellPane,
  shouldIgnoreMobileSwipe,
  swipeDeltaToDirection,
  type MobileShellMode,
  type MobileShellPane,
} from '@/components/layout/mobile-shell-layout'

type MobileThreePaneProps = {
  enabled: boolean
  mode: MobileShellMode | 'off'
  pane: MobileShellPane
  onPaneChange: (pane: MobileShellPane) => void
  sessions: ReactNode
  chat: ReactNode
  context: ReactNode
}

export function MobileThreePane({
  enabled,
  mode,
  pane,
  onPaneChange,
  sessions,
  chat,
  context,
}: MobileThreePaneProps) {
  const { t } = useI18n()
  const trackRef = useRef<HTMLDivElement>(null)
  const paneRef = useRef(pane)
  paneRef.current = pane
  const onPaneChangeRef = useRef(onPaneChange)
  onPaneChangeRef.current = onPaneChange

  useEffect(() => {
    const node = trackRef.current
    if (!node || mode !== 'phone') return
    let startX = 0
    let startY = 0
    let tracking = false
    let locked = false
    const start = (event: TouchEvent) => {
      if (event.touches.length !== 1) return
      if (shouldIgnoreMobileSwipe(event.target)) {
        tracking = false
        return
      }
      tracking = true
      locked = false
      startX = event.touches[0]?.clientX ?? 0
      startY = event.touches[0]?.clientY ?? 0
    }
    const move = (event: TouchEvent) => {
      if (!tracking || event.touches.length !== 1) return
      const dx = (event.touches[0]?.clientX ?? startX) - startX
      const dy = (event.touches[0]?.clientY ?? startY) - startY
      if (!locked) {
        if (Math.abs(dx) < 10 && Math.abs(dy) < 10) return
        if (Math.abs(dx) <= Math.abs(dy)) {
          tracking = false
          return
        }
        locked = true
      }
      event.preventDefault()
    }
    const end = (event: TouchEvent) => {
      if (!tracking) return
      tracking = false
      if (!locked) return
      const touch = event.changedTouches[0]
      if (!touch) return
      const direction = swipeDeltaToDirection(touch.clientX - startX, touch.clientY - startY)
      if (!direction) return
      onPaneChangeRef.current(moveMobileShellPane(paneRef.current, direction))
    }
    const cancel = () => {
      tracking = false
    }
    node.addEventListener('touchstart', start, { passive: true })
    node.addEventListener('touchmove', move, { passive: false })
    node.addEventListener('touchend', end)
    node.addEventListener('touchcancel', cancel)
    return () => {
      node.removeEventListener('touchstart', start)
      node.removeEventListener('touchmove', move)
      node.removeEventListener('touchend', end)
      node.removeEventListener('touchcancel', cancel)
    }
  }, [mode])

  if (!enabled || mode === 'off') {
    return (
      <>
        {sessions}
        {chat}
      </>
    )
  }

  const panes = [
    { id: 'sessions' as const, label: t('navigation:mobileShell.sessions'), node: sessions },
    { id: 'chat' as const, label: t('navigation:mobileShell.chat'), node: chat },
    { id: 'context' as const, label: t('navigation:mobileShell.context'), node: context },
  ]
  const phone = mode === 'phone'

  return (
    <div
      className={cn(
        'relative h-full min-h-0 min-w-0 flex-1 overflow-hidden',
        phone
          ? 'flex flex-col'
          : 'grid min-h-0 grid-cols-[minmax(220px,260px)_minmax(0,1fr)_minmax(280px,360px)] grid-rows-[minmax(0,1fr)]',
      )}
      data-mobile-shell={mode}
    >
      {phone && (
        <div
          className="[[data-mobile-keyboard='open']_&]:hidden flex h-7 flex-none items-center justify-center gap-1 border-b border-[var(--stroke-soft)] bg-[var(--sidebar-bg)] px-2"
          role="tablist"
          aria-label={t('navigation:mobileShell.pager')}
        >
          {panes.map((item) => {
            const selected = pane === item.id
            return (
              <button
                key={item.id}
                type="button"
                role="tab"
                aria-selected={selected}
                className={cn(
                  'h-6 rounded-full px-2.5 text-[11px] font-medium',
                  selected
                    ? 'bg-[var(--star-soft)] text-[var(--star-strong)]'
                    : 'text-[var(--text-muted)]',
                )}
                onClick={() => onPaneChange(item.id)}
              >
                {item.label}
              </button>
            )
          })}
        </div>
      )}
      <div
        ref={trackRef}
        className={cn(
          'min-h-0 min-w-0 flex-1 touch-pan-y',
          phone ? 'flex w-[300%] transition-transform duration-200 ease-out' : 'contents',
        )}
        style={
          phone
            ? { transform: `translateX(-${mobileShellPaneIndex(pane) * (100 / 3)}%)` }
            : undefined
        }
      >
        {panes.map((item) => (
          <section
            key={item.id}
            className={cn(
              'flex h-full min-h-0 min-w-0 flex-col overflow-hidden',
              phone ? 'w-1/3' : 'min-w-0',
            )}
            aria-label={item.label}
            aria-hidden={phone && pane !== item.id ? true : undefined}
            {...(phone && pane !== item.id ? { inert: true } : {})}
          >
            {item.node}
          </section>
        ))}
      </div>
    </div>
  )
}
