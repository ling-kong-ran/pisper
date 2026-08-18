// 装饰动画组件：指针悬浮在元素上时显示准星/光环。
import { useCallback, useRef, type FocusEvent, type PointerEvent, type ReactNode } from 'react'
import { cn } from '@/lib/utils'
import './react-bits.css'

type TargetCursorProps = {
  children: ReactNode
  className?: string
  targetSelector?: string
}

export function TargetCursor({
  children,
  className,
  targetSelector = '[data-target-cursor]',
}: TargetCursorProps) {
  const rootRef = useRef<HTMLDivElement>(null)
  const cursorRef = useRef<HTMLSpanElement>(null)
  const frameRef = useRef<HTMLSpanElement>(null)

  const motionAllowed = useCallback(
    () =>
      window.matchMedia('(pointer: fine)').matches &&
      !window.matchMedia('(prefers-reduced-motion: reduce)').matches,
    [],
  )

  const positionFrame = useCallback((target: Element) => {
    const root = rootRef.current
    const frame = frameRef.current
    if (!root || !frame) return
    const rootRect = root.getBoundingClientRect()
    const targetRect = target.getBoundingClientRect()
    const padding = 7
    frame.style.transform = `translate3d(${targetRect.left - rootRect.left - padding}px, ${targetRect.top - rootRect.top - padding}px, 0)`
    frame.style.width = `${targetRect.width + padding * 2}px`
    frame.style.height = `${targetRect.height + padding * 2}px`
    frame.dataset.visible = 'true'
  }, [])

  const handlePointerMove = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      if (!motionAllowed()) return
      const root = rootRef.current
      const cursor = cursorRef.current
      const frame = frameRef.current
      if (!root || !cursor || !frame) return
      const rootRect = root.getBoundingClientRect()
      cursor.style.transform = `translate3d(${event.clientX - rootRect.left}px, ${event.clientY - rootRect.top}px, 0)`
      cursor.dataset.visible = 'true'
      const target = (event.target as Element | null)?.closest(targetSelector)
      if (target && root.contains(target)) positionFrame(target)
      else frame.dataset.visible = 'false'
    },
    [motionAllowed, positionFrame, targetSelector],
  )

  const handlePointerLeave = useCallback(() => {
    if (cursorRef.current) cursorRef.current.dataset.visible = 'false'
    if (frameRef.current) frameRef.current.dataset.visible = 'false'
  }, [])

  const handleFocus = useCallback(
    (event: FocusEvent<HTMLDivElement>) => {
      const target = (event.target as Element | null)?.closest(targetSelector)
      if (target && rootRef.current?.contains(target)) positionFrame(target)
    },
    [positionFrame, targetSelector],
  )

  const handleBlur = useCallback((event: FocusEvent<HTMLDivElement>) => {
    const nextTarget = event.relatedTarget
    if (nextTarget instanceof Element && rootRef.current?.contains(nextTarget)) return
    if (frameRef.current) frameRef.current.dataset.visible = 'false'
  }, [])

  return (
    <div
      ref={rootRef}
      className={cn('rb-target-cursor', className)}
      onPointerMove={handlePointerMove}
      onPointerLeave={handlePointerLeave}
      onFocusCapture={handleFocus}
      onBlurCapture={handleBlur}
    >
      {children}
      <span ref={cursorRef} className="rb-target-cursor-dot" aria-hidden="true" />
      <span ref={frameRef} className="rb-target-cursor-frame" aria-hidden="true">
        <i />
        <i />
        <i />
        <i />
      </span>
    </div>
  )
}
