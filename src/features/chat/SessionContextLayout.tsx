// 局部分栏只有一个尺寸所有者；临时空间不足仅约束显示，不能覆盖用户的宽度偏好。
import { useEffect, useRef, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import {
  SESSION_CONTEXT_DEFAULT_WIDTH,
  SESSION_CONTEXT_MIN_WIDTH,
  SESSION_CONTEXT_MAX_WIDTH,
  sessionContextDisplayWidth,
  type SessionContextPresentation,
} from './session-context-layout'

type SessionContextLayoutProps = {
  preferredWidth: number
  onWidthChange: (width: number) => void
  availableWidth: number
  presentation: SessionContextPresentation
  children: ReactNode
  context: ReactNode
  side?: 'left' | 'right'
  layoutId?: string
  conversationHeaderInset?: number
}
export function SessionContextLayout({
  preferredWidth: preferred,
  onWidthChange: setWidth,
  availableWidth,
  presentation,
  children,
  context,
  side = 'right',
  layoutId = 'chat',
  conversationHeaderInset = 0,
}: SessionContextLayoutProps) {
  const { t } = useI18n()
  const cleanup = useRef<() => void>(() => {})
  useEffect(() => () => cleanup.current(), [])
  const aside = presentation === 'aside'
  const contentOnly = presentation === 'content'
  const width = sessionContextDisplayWidth(preferred, availableWidth)
  const maximum = sessionContextDisplayWidth(SESSION_CONTEXT_MAX_WIDTH, availableWidth)
  const setDisplayedWidth = (next: number) =>
    setWidth(Math.max(SESSION_CONTEXT_MIN_WIDTH, Math.min(maximum, next)))
  const beginResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    cleanup.current()
    event.preventDefault()
    event.currentTarget.focus()
    event.currentTarget.setPointerCapture(event.pointerId)
    const startX = event.clientX
    const move = (next: PointerEvent) =>
      setDisplayedWidth(width + (next.clientX - startX) * (side === 'left' ? 1 : -1))
    const finish = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', finish)
      window.removeEventListener('pointercancel', cancel)
      window.removeEventListener('keydown', key)
    }
    const cancel = () => {
      setWidth(preferred)
      finish()
    }
    const key = (next: KeyboardEvent) => {
      if (next.key === 'Escape') {
        next.preventDefault()
        next.stopPropagation()
        cancel()
      }
    }
    cleanup.current = finish
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', finish, { once: true })
    window.addEventListener('pointercancel', cancel, { once: true })
    window.addEventListener('keydown', key)
  }
  const chat = (
    <div
      key="conversation"
      id={`${layoutId}-conversation`}
      data-workspace-conversation
      inert={contentOnly}
      className={contentOnly ? 'hidden' : 'min-h-0 min-w-0 flex-1 overflow-hidden'}
      style={aside && conversationHeaderInset ? { paddingTop: conversationHeaderInset } : undefined}
    >
      {children}
    </div>
  )
  const separator = aside ? (
    <div
      key="separator"
      role="separator"
      tabIndex={0}
      aria-orientation="vertical"
      aria-label={t('chat:sessionContext.resize')}
      title={t('chat:sessionContext.resizeHint')}
      aria-controls={`${layoutId}-conversation ${layoutId}-content`}
      aria-valuemin={SESSION_CONTEXT_MIN_WIDTH}
      aria-valuemax={maximum}
      aria-valuenow={width}
      onPointerDown={beginResize}
      onDoubleClick={() => setWidth(SESSION_CONTEXT_DEFAULT_WIDTH)}
      onKeyDown={(event) => {
        if (!['ArrowLeft', 'ArrowRight', 'Home', 'End', 'Enter'].includes(event.key)) return
        event.preventDefault()
        if (event.key === 'Enter') {
          setWidth(SESSION_CONTEXT_DEFAULT_WIDTH)
          return
        }
        const step = event.shiftKey ? 64 : 16
        setDisplayedWidth(
          event.key === 'Home'
            ? SESSION_CONTEXT_MIN_WIDTH
            : event.key === 'End'
              ? maximum
              : width + (event.key === 'ArrowRight' ? step : -step) * (side === 'left' ? 1 : -1),
        )
      }}
      className="group flex w-2 shrink-0 cursor-col-resize touch-none items-center justify-center outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
    >
      <span
        aria-hidden="true"
        className="h-10 w-0.5 rounded-full bg-border group-hover:bg-ring group-focus-visible:bg-ring"
      />
    </div>
  ) : null
  const resource = (
    <div
      key="resource"
      id={`${layoutId}-content`}
      className={
        aside
          ? 'h-full min-h-0 shrink-0 overflow-hidden'
          : contentOnly
            ? 'h-full min-h-0 min-w-0 flex-1 overflow-hidden'
            : 'hidden'
      }
      style={aside ? { width } : undefined}
    >
      {(aside || contentOnly) && context}
    </div>
  )
  return (
    <>
      <div
        id={`${layoutId}-context-layout`}
        data-workspace-layout
        className="flex h-full min-h-0 min-w-0 flex-1 overflow-hidden"
      >
        {side === 'left' ? [resource, separator, chat] : [chat, separator, resource]}
      </div>
      {presentation === 'sheet' && context}
    </>
  )
}
