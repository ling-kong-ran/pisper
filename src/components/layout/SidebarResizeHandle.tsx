import { useEffect, useRef, useState, type PointerEvent } from 'react'
import { clampSidebarWidth, MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH } from '@/lib/sidebar-width'
import { cn } from '@/lib/utils'

type SidebarResizeHandleProps = {
  width: number
  label: string
  onPreview: (width: number | null) => void
  onCommit: (width: number) => void
}

export function SidebarResizeHandle({
  width,
  label,
  onPreview,
  onCommit,
}: SidebarResizeHandleProps) {
  const drag = useRef<{ id: number; x: number; width: number; next: number } | null>(null)
  const [dragging, setDragging] = useState(false)
  const previewRef = useRef(onPreview)
  previewRef.current = onPreview
  useEffect(() => () => previewRef.current(null), [])
  const limit = (value: number) => clampSidebarWidth(value, window.innerWidth - 52)
  const finish = (event: PointerEvent<HTMLDivElement>, commit: boolean) => {
    if (drag.current?.id !== event.pointerId) return
    const value = drag.current.next
    drag.current = null
    setDragging(false)
    if (commit) onCommit(value)
    onPreview(null)
    if (event.currentTarget.hasPointerCapture(event.pointerId))
      event.currentTarget.releasePointerCapture(event.pointerId)
  }

  return (
    <div
      role="separator"
      tabIndex={0}
      aria-label={label}
      aria-orientation="vertical"
      aria-controls="workbench-sidebar"
      aria-valuemin={MIN_SIDEBAR_WIDTH}
      aria-valuemax={limit(MAX_SIDEBAR_WIDTH)}
      aria-valuenow={Math.round(width)}
      data-testid="sidebar-resize-handle"
      className={cn(
        'absolute inset-y-0 right-0 z-20 w-1.5 touch-none cursor-col-resize select-none outline-none after:absolute after:inset-y-3 after:right-0 after:w-0.5 after:rounded-full after:transition-colors hover:after:bg-border focus-visible:after:bg-ring',
        dragging && 'after:bg-ring',
      )}
      onPointerDown={(event) => {
        if (event.button !== 0) return
        event.preventDefault()
        event.currentTarget.focus()
        event.currentTarget.setPointerCapture(event.pointerId)
        drag.current = { id: event.pointerId, x: event.clientX, width, next: width }
        setDragging(true)
      }}
      onPointerMove={(event) => {
        const current = drag.current
        if (!current || current.id !== event.pointerId) return
        current.next = limit(current.width + event.clientX - current.x)
        onPreview(current.next)
      }}
      onPointerUp={(event) => finish(event, true)}
      onPointerCancel={(event) => finish(event, false)}
      onLostPointerCapture={(event) => finish(event, false)}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && drag.current) {
          event.preventDefault()
          const id = drag.current.id
          drag.current = null
          setDragging(false)
          onPreview(null)
          if (event.currentTarget.hasPointerCapture(id))
            event.currentTarget.releasePointerCapture(id)
          return
        }
        const step = event.shiftKey ? 40 : 10
        const next =
          event.key === 'ArrowLeft'
            ? width - step
            : event.key === 'ArrowRight'
              ? width + step
              : event.key === 'Home'
                ? MIN_SIDEBAR_WIDTH
                : event.key === 'End'
                  ? MAX_SIDEBAR_WIDTH
                  : null
        if (next === null) return
        event.preventDefault()
        onCommit(limit(next))
      }}
    />
  )
}
