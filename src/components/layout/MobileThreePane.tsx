// 手机和平板均一次显示一屏，通过底部导航和手势切换，旋转时保留页面状态。
import { useEffect, useRef, type KeyboardEvent, type ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'
import { List, MessageSquare, PanelsTopLeft } from 'lucide-react'
import {
  moveMobileShellPane,
  shouldIgnoreMobileSwipe,
  swipeDeltaToDirection,
  type MobileShellMode,
  type MobileShellPane,
} from '@/components/layout/mobile-shell-layout'

export type MobileThreePaneProps = {
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
    if (!node || !enabled || mode === 'off') return
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
  }, [enabled, mode])

  const panes = [
    {
      id: 'sessions' as const,
      label: t('navigation:mobileShell.sessions'),
      icon: List,
      node: sessions,
    },
    {
      id: 'chat' as const,
      label: t('navigation:mobileShell.chat'),
      icon: MessageSquare,
      node: chat,
    },
    {
      id: 'context' as const,
      label: t('navigation:mobileShell.context'),
      icon: PanelsTopLeft,
      node: context,
    },
  ]
  const desktop = !enabled || mode === 'off'
  const paged = !desktop
  const onPagerKeyDown = (event: KeyboardEvent<HTMLButtonElement>, current: MobileShellPane) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
    const next =
      event.key === 'Home'
        ? 'sessions'
        : event.key === 'End'
          ? 'context'
          : moveMobileShellPane(current, event.key === 'ArrowLeft' ? -1 : 1)
    event.preventDefault()
    onPaneChange(next)
    requestAnimationFrame(() => document.getElementById(`mobile-shell-tab-${next}`)?.focus())
  }

  return (
    <div
      className={cn(
        'relative h-full min-h-0 min-w-0 flex-1 overflow-clip',
        desktop ? 'contents' : 'flex flex-col',
      )}
      data-mobile-shell={mode}
    >
      {paged && (
        <div
          className="[[data-mobile-keyboard='open']_&]:hidden order-last flex min-h-14 flex-none items-center justify-center gap-1 border-t border-[var(--stroke-soft)] bg-[var(--sidebar-bg)] px-2 py-1"
          data-mobile-navigation="panes"
          role="tablist"
          aria-label={t('navigation:mobileShell.pager')}
        >
          {panes.map((item) => {
            const selected = pane === item.id
            return (
              <button
                key={item.id}
                id={`mobile-shell-tab-${item.id}`}
                type="button"
                role="tab"
                aria-selected={selected}
                aria-controls={`mobile-shell-pane-${item.id}`}
                tabIndex={selected ? 0 : -1}
                className={cn(
                  'flex h-12 min-w-0 flex-1 flex-col items-center justify-center gap-1 rounded-xl px-2 text-[11px] font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
                  selected
                    ? 'bg-[var(--star-soft)] text-[var(--star-strong)]'
                    : 'text-[var(--text-muted)]',
                )}
                onClick={() => onPaneChange(item.id)}
                onKeyDown={(event) => onPagerKeyDown(event, item.id)}
              >
                <item.icon className="size-5" aria-hidden="true" />
                <span className="truncate">{item.label}</span>
              </button>
            )
          })}
        </div>
      )}
      {/* 只切换显示状态，不移动轨道；非当前屏保留组件状态但退出布局。 */}
      <div
        ref={trackRef}
        className={cn('min-h-0 min-w-0 flex-1 touch-pan-y', paged ? 'block w-full' : 'contents')}
      >
        {panes.map((item) => (
          <section
            key={item.id}
            id={`mobile-shell-pane-${item.id}`}
            role={paged ? 'tabpanel' : undefined}
            className={cn(
              // 所有断点保留同一棵页面树；只改布局，避免丢失尚未保存的表单。
              desktop
                ? item.id === 'context'
                  ? 'hidden'
                  : 'contents'
                : pane !== item.id
                  ? 'hidden'
                  : 'flex h-full min-h-0 min-w-0 flex-col overflow-hidden',
              paged && 'w-full',
            )}
            aria-label={item.label}
            aria-labelledby={paged ? `mobile-shell-tab-${item.id}` : undefined}
            aria-hidden={paged && pane !== item.id ? true : undefined}
            {...(paged && pane !== item.id ? { inert: true } : {})}
          >
            {item.node}
          </section>
        ))}
      </div>
    </div>
  )
}
