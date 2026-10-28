import { useEffect, useRef, useState, type PointerEvent } from 'react'
import {
  clampSidebarWidth,
  DEFAULT_SIDEBAR_WIDTH,
  MAX_SIDEBAR_WIDTH,
  MIN_SIDEBAR_WIDTH,
} from '@/lib/sidebar-width'
import { cn } from '@/lib/utils'

// 拖过最小宽度再多拖这段距离才收起，避免调窄时误触。
const COLLAPSE_SLACK = 72

type SidebarResizeHandleProps = {
  width: number
  label: string
  hint: string
  onPreview: (width: number | null) => void
  onCommit: (width: number) => void
  // 设置页侧栏常驻，不提供收起。
  onCollapse?: () => void
}

type DragState = { id: number; x: number; width: number; next: number; collapse: boolean }

// 拖动期间光标和禁止选中作用于整页，快速划过正文时不闪回文本光标。
function setResizingCursor(active: boolean) {
  document.documentElement.toggleAttribute('data-sidebar-resizing', active)
}

export function SidebarResizeHandle({
  width,
  label,
  hint,
  onPreview,
  onCommit,
  onCollapse,
}: SidebarResizeHandleProps) {
  const drag = useRef<DragState | null>(null)
  const [dragging, setDragging] = useState(false)
  const [collapsing, setCollapsing] = useState(false)
  const previewRef = useRef(onPreview)
  previewRef.current = onPreview
  useEffect(
    () => () => {
      previewRef.current(null)
      setResizingCursor(false)
    },
    [],
  )
  const limit = (value: number) => clampSidebarWidth(value, window.innerWidth - 52)
  const reset = () => {
    drag.current = null
    setDragging(false)
    setCollapsing(false)
    setResizingCursor(false)
  }
  const finish = (event: PointerEvent<HTMLDivElement>, commit: boolean) => {
    const current = drag.current
    if (current?.id !== event.pointerId) return
    reset()
    onPreview(null)
    if (event.currentTarget.hasPointerCapture(event.pointerId))
      event.currentTarget.releasePointerCapture(event.pointerId)
    if (!commit) return
    // 收起时保留原宽度偏好，重新展开仍回到用户习惯的宽度。
    if (current.collapse && onCollapse) onCollapse()
    else if (current.next !== current.width) onCommit(current.next)
  }

  return (
    <div
      role="separator"
      tabIndex={0}
      aria-label={label}
      title={hint}
      aria-orientation="vertical"
      aria-controls="workbench-sidebar"
      aria-valuemin={MIN_SIDEBAR_WIDTH}
      aria-valuemax={limit(MAX_SIDEBAR_WIDTH)}
      aria-valuenow={Math.round(width)}
      data-testid="sidebar-resize-handle"
      data-dragging={dragging || undefined}
      className={cn(
        'group/resize absolute inset-y-0 right-0 z-20 w-2 touch-none cursor-col-resize select-none outline-none',
        'after:absolute after:inset-y-2 after:right-0 after:w-px after:rounded-full after:bg-transparent after:transition-[background-color,width] after:duration-150',
        'hover:after:w-0.5 hover:after:bg-[var(--stroke)] hover:after:delay-150 focus-visible:after:w-0.5 focus-visible:after:bg-[var(--focus)]',
        dragging && 'after:w-0.5 after:bg-[var(--focus)] after:delay-0',
        collapsing && 'after:bg-[var(--text-muted)]',
      )}
      onPointerDown={(event) => {
        if (event.button !== 0) return
        event.preventDefault()
        event.currentTarget.focus()
        event.currentTarget.setPointerCapture(event.pointerId)
        drag.current = {
          id: event.pointerId,
          x: event.clientX,
          width,
          next: width,
          collapse: false,
        }
        setDragging(true)
        setResizingCursor(true)
      }}
      onPointerMove={(event) => {
        const current = drag.current
        if (!current || current.id !== event.pointerId) return
        const raw = current.width + event.clientX - current.x
        current.collapse = Boolean(onCollapse) && raw < MIN_SIDEBAR_WIDTH - COLLAPSE_SLACK
        setCollapsing(current.collapse)
        current.next = limit(raw)
        onPreview(current.next)
      }}
      onPointerUp={(event) => finish(event, true)}
      onPointerCancel={(event) => finish(event, false)}
      onLostPointerCapture={(event) => finish(event, false)}
      onDoubleClick={() => onCommit(limit(DEFAULT_SIDEBAR_WIDTH))}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && drag.current) {
          event.preventDefault()
          const id = drag.current.id
          reset()
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
                  : event.key === 'Enter'
                    ? DEFAULT_SIDEBAR_WIDTH
                    : null
        if (next === null) return
        event.preventDefault()
        onCommit(limit(next))
      }}
    />
  )
}
