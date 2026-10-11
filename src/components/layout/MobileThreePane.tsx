// 手机一次只显示一屏，默认停在对话；Pad 横屏同时放下左中右，不再占用底栏高度。
import { useId, type ReactNode } from 'react'
import { ChevronLeft, ChevronRight } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useI18n } from '@/app/i18n/use-i18n'
import { cn } from '@/lib/utils'
import {
  mobileShellPaneIndex,
  moveMobileShellPane,
  type MobileShellMode,
  type MobileShellPane,
} from '@/components/layout/mobile-shell-layout'
import { useMobilePaneTransition } from '@/components/layout/useMobilePaneTransition'

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
  const phone = enabled && mode === 'phone'
  const trackRef = useMobilePaneTransition(phone, pane, onPaneChange)
  const panelId = useId()

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
        <div className="[[data-mobile-keyboard='open']_&]:hidden flex h-11 flex-none items-center gap-1 border-b border-[var(--stroke-soft)] bg-[var(--sidebar-bg)] px-1">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-11 shrink-0 disabled:opacity-30"
            aria-label={t('navigation:mobileShell.previousPane')}
            title={t('navigation:mobileShell.previousPane')}
            disabled={pane === 'sessions'}
            onClick={() => onPaneChange(moveMobileShellPane(pane, -1))}
          >
            <ChevronLeft size={18} aria-hidden="true" />
          </Button>
          <div
            className="flex min-w-0 flex-1 items-center justify-center gap-1"
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
                  aria-controls={`${panelId}-${item.id}`}
                  tabIndex={selected ? 0 : -1}
                  onKeyDown={(event) => {
                    const index = mobileShellPaneIndex(pane)
                    const next =
                      event.key === 'ArrowRight'
                        ? (index + 1) % panes.length
                        : event.key === 'ArrowLeft'
                          ? (index + panes.length - 1) % panes.length
                          : event.key === 'Home'
                            ? 0
                            : event.key === 'End'
                              ? panes.length - 1
                              : -1
                    if (next < 0) return
                    event.preventDefault()
                    onPaneChange(panes[next].id)
                    const tabs = event.currentTarget.parentElement?.querySelectorAll('button')
                    tabs?.[next]?.focus()
                  }}
                  className={cn(
                    'h-9 min-w-0 truncate rounded-full px-2.5 text-[11px] font-medium',
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
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-11 shrink-0 disabled:opacity-30"
            aria-label={t('navigation:mobileShell.nextPane')}
            title={t('navigation:mobileShell.nextPane')}
            disabled={pane === 'context'}
            onClick={() => onPaneChange(moveMobileShellPane(pane, 1))}
          >
            <ChevronRight size={18} aria-hidden="true" />
          </Button>
        </div>
      )}
      {/* 翻页变换只存在于手势/动画期间，静止后由 Hook 清理。 */}
      <div
        ref={trackRef}
        className={cn('min-h-0 min-w-0 flex-1', phone ? 'flex w-[300%]' : 'contents')}
      >
        {panes.map((item) => (
          <section
            key={item.id}
            id={`${panelId}-${item.id}`}
            role={phone ? 'tabpanel' : undefined}
            className={cn(
              'flex h-full min-h-0 min-w-0 flex-col overflow-hidden',
              phone ? 'w-1/3 shrink-0 bg-background' : 'min-w-0',
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
