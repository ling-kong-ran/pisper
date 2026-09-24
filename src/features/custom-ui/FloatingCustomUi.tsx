import { GripVertical, RotateCcw, X } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import type { KeyboardEvent, PointerEvent, RefObject } from 'react'
import { useI18n } from '@/app/use-i18n'
import { useIsMobile } from '@/hooks/use-mobile'
import { cn } from '@/lib/utils'
import { CustomUiWidget } from './CustomUiWidget'
import {
  defaultFloatingPlacement,
  floatingPlacementKey,
  floatingPositionPixels,
  moveFloatingPlacement,
  type FloatingPlacement,
  type FloatingSize,
  type FloatingAnchor,
} from './floating-placement'
import { useFloatingPlacementStore } from './floating-placement-store'

type DragState = {
  pointerId: number
  element: HTMLButtonElement
  x: number
  y: number
  initial: FloatingPlacement
  latest: FloatingPlacement
}

const EMPTY_SIZE = { width: 0, height: 0 }

function FloatingWidget({
  id,
  componentId,
  boundaryRef,
  anchorRef,
  notify,
  onClose,
  stackIndex,
}: {
  id: string
  componentId: string
  boundaryRef: RefObject<HTMLDivElement | null>
  anchorRef?: RefObject<HTMLElement | null>
  notify?: (message: string) => void
  onClose?: (componentId: string) => void
  stackIndex: number
}) {
  const { t } = useI18n()
  const mobile = useIsMobile()
  const key = floatingPlacementKey(id, mobile)
  const saved = useFloatingPlacementStore(
    (state) => state.positions.find((item) => item.key === key)?.position,
  )
  const elementRef = useRef<HTMLDivElement>(null)
  const dragRef = useRef<DragState | null>(null)
  const [draft, setDraft] = useState<FloatingPlacement | null>(null)
  const [sizes, setSizes] = useState<{
    bounds: FloatingSize
    island: FloatingSize
    anchor?: FloatingAnchor
  }>({
    bounds: EMPTY_SIZE,
    island: EMPTY_SIZE,
  })
  const position =
    saved ?? defaultFloatingPlacement(sizes.bounds, sizes.island, sizes.anchor, stackIndex)
  const helpId = useId()

  useEffect(() => {
    const boundary = boundaryRef.current
    const element = elementRef.current
    if (!boundary || !element) return
    let observedAnchor: HTMLElement | null = null
    const observer = new ResizeObserver(() => measure())
    let pendingAnchor: MutationObserver | null = null
    const measure = () => {
      const anchor = anchorRef?.current
      if (anchor !== observedAnchor) {
        if (observedAnchor) observer.unobserve(observedAnchor)
        observedAnchor = anchor ?? null
        if (anchor) {
          observer.observe(anchor)
          pendingAnchor?.disconnect()
          pendingAnchor = null
        }
      }
      const boundaryRect = boundary.getBoundingClientRect()
      const anchorRect = anchor?.getBoundingClientRect()
      const next = {
        bounds: { width: boundary.clientWidth, height: boundary.clientHeight },
        island: { width: element.offsetWidth, height: element.offsetHeight },
        anchor:
          anchorRect && anchorRect.width > 0 && anchorRect.height > 0
            ? {
                x: anchorRect.left - boundaryRect.left,
                y: anchorRect.top - boundaryRect.top,
                width: anchorRect.width,
                height: anchorRect.height,
              }
            : undefined,
      }
      setSizes((current) =>
        current.bounds.width === next.bounds.width &&
        current.bounds.height === next.bounds.height &&
        current.island.width === next.island.width &&
        current.island.height === next.island.height &&
        current.anchor?.x === next.anchor?.x &&
        current.anchor?.y === next.anchor?.y &&
        current.anchor?.width === next.anchor?.width &&
        current.anchor?.height === next.anchor?.height
          ? current
          : next,
      )
    }
    measure()
    observer.observe(boundary)
    observer.observe(element)
    // 页面头部可以懒加载；只在初次锚点尚未挂载期间等待它出现。
    if (anchorRef && !anchorRef.current) {
      pendingAnchor = new MutationObserver(() => {
        if (anchorRef.current) measure()
      })
      pendingAnchor.observe(document.body, { childList: true, subtree: true })
    }
    window.addEventListener('resize', measure)
    window.addEventListener('scroll', measure, true)
    return () => {
      observer.disconnect()
      pendingAnchor?.disconnect()
      window.removeEventListener('resize', measure)
      window.removeEventListener('scroll', measure, true)
    }
  }, [boundaryRef, anchorRef])

  useEffect(() => {
    setDraft(null)
    return () => {
      const drag = dragRef.current
      dragRef.current = null
      if (drag?.element.hasPointerCapture(drag.pointerId))
        drag.element.releasePointerCapture(drag.pointerId)
    }
  }, [key])

  const finish = (event: PointerEvent<HTMLButtonElement>, commit: boolean) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    dragRef.current = null
    if (commit) useFloatingPlacementStore.getState().setPosition(key, drag.latest)
    setDraft(null)
    if (drag.element.hasPointerCapture(drag.pointerId))
      drag.element.releasePointerCapture(drag.pointerId)
  }
  const reset = () => useFloatingPlacementStore.getState().resetPosition(key)
  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const step = event.shiftKey ? 32 : 12
    const moves: Record<string, FloatingPlacement> = {
      ArrowLeft: { x: -step, y: 0 },
      ArrowRight: { x: step, y: 0 },
      ArrowUp: { x: 0, y: -step },
      ArrowDown: { x: 0, y: step },
    }
    if (event.key === 'Home') {
      event.preventDefault()
      reset()
      return
    }
    const delta = moves[event.key]
    if (!delta) return
    event.preventDefault()
    useFloatingPlacementStore
      .getState()
      .setPosition(key, moveFloatingPlacement(position, delta, sizes.bounds, sizes.island))
  }
  const pixels = floatingPositionPixels(draft ?? position, sizes.bounds, sizes.island)
  return (
    <div
      ref={elementRef}
      data-floating-widget={id}
      className={cn(
        'pointer-events-auto absolute flex w-[380px] max-w-full flex-col',
        componentId === 'pisper-island' && 'max-[650px]:w-[210px] max-[650px]:px-5',
      )}
      style={{
        left: pixels.x,
        top: pixels.y,
        maxHeight: '100%',
        visibility: sizes.bounds.width > 0 ? 'visible' : 'hidden',
      }}
    >
      <div className="absolute left-0 top-1/2 z-10 flex -translate-y-1/2 flex-col items-center gap-1">
        <button
          type="button"
          aria-label={t('custom-ui:floating.move')}
          aria-describedby={helpId}
          title={t('custom-ui:floating.moveHint')}
          className="flex h-6 w-5 touch-none items-center justify-center rounded-full bg-background/85 text-muted-foreground shadow-sm outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
          style={{ cursor: draft ? 'grabbing' : 'grab' }}
          onKeyDown={onKeyDown}
          onPointerDown={(event) => {
            if (event.button !== 0 || dragRef.current) return
            event.preventDefault()
            event.currentTarget.focus({ preventScroll: true })
            event.currentTarget.setPointerCapture(event.pointerId)
            dragRef.current = {
              pointerId: event.pointerId,
              element: event.currentTarget,
              x: event.clientX,
              y: event.clientY,
              initial: position,
              latest: position,
            }
            setDraft(position)
          }}
          onPointerMove={(event) => {
            const drag = dragRef.current
            if (!drag || drag.pointerId !== event.pointerId) return
            const next = moveFloatingPlacement(
              drag.initial,
              { x: event.clientX - drag.x, y: event.clientY - drag.y },
              sizes.bounds,
              sizes.island,
            )
            drag.latest = next
            setDraft(next)
          }}
          onPointerUp={(event) => finish(event, true)}
          onPointerCancel={(event) => finish(event, false)}
          onLostPointerCapture={(event) => finish(event, false)}
        >
          <GripVertical className="size-3" />
        </button>
        <button
          type="button"
          aria-label={t('custom-ui:floating.resetPosition')}
          title={t('custom-ui:floating.resetPosition')}
          onClick={reset}
          className="flex size-5 items-center justify-center rounded-full bg-background/85 text-muted-foreground shadow-sm outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
        >
          <RotateCcw className="size-3" />
        </button>
        <span id={helpId} className="sr-only">
          {t('custom-ui:floating.moveHint')}
        </span>
      </div>
      {onClose && (
        <button
          type="button"
          aria-label={t('custom-ui:floating.close')}
          title={t('custom-ui:floating.close')}
          onClick={() => onClose(componentId)}
          className="absolute right-0 top-1/2 z-10 flex size-5 -translate-y-1/2 items-center justify-center rounded-full bg-background/85 text-muted-foreground shadow-sm outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
        >
          <X className="size-3" />
        </button>
      )}
      <div
        className="flex min-h-0 flex-col overflow-hidden"
        style={{ height: componentId === 'pisper-island' ? 64 : 240 }}
      >
        <CustomUiWidget componentId={componentId} notify={notify} />
      </div>
    </div>
  )
}

export function FloatingCustomUi({
  widgets,
  notify,
  anchorRef,
  onClose,
}: {
  widgets: readonly { id: string; componentId: string }[]
  notify?: (message: string) => void
  anchorRef?: RefObject<HTMLElement | null>
  onClose?: (componentId: string) => void
}) {
  const boundaryRef = useRef<HTMLDivElement>(null)
  const floatingOrder = widgets.filter((widget) => widget.componentId !== 'pisper-island')
  return (
    <div
      ref={boundaryRef}
      className="pointer-events-none fixed inset-x-2 bottom-2 top-0 z-40 overflow-hidden"
      data-floating-custom-ui
    >
      {widgets.map((widget) => (
        <FloatingWidget
          key={widget.id}
          id={widget.id}
          componentId={widget.componentId}
          boundaryRef={boundaryRef}
          notify={notify}
          anchorRef={anchorRef}
          onClose={onClose}
          stackIndex={
            widget.componentId === 'pisper-island'
              ? 0
              : floatingOrder.findIndex((item) => item.id === widget.id) + 1
          }
        />
      ))}
    </div>
  )
}
