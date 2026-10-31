// 浮窗放置宿主（规格 5.5 floating，计划 §4.5 第 5 项）：由旧的 FloatingCustomUi 改写。
// 每个 floating 实例一张可拖动的浮层，尺寸取 manifest 的 placements[floating].size（缺省 320×200），
// 位置继续存在 pisper-floating-placement（节点 id = instanceId）；关闭 = 实例 PATCH enabled:false（可在工作室重新打开）。
// 边界层从标题栏下沿（48px）开始，不挡住桌面自定义标题栏右上的窗口按钮。手机外壳不渲染（负责人决定，由轻量层判断）。
import { GripVertical, RotateCcw, X } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import type { KeyboardEvent, PointerEvent, RefObject } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { FLOATING_DEFAULT_SIZE, manifestPlacement } from '@shared/custom-ui-manifest.mjs'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'
import { updateComponentInstance } from '@/features/custom-ui/custom-ui-api'
import {
  defaultFloatingPlacement,
  floatingPlacementKey,
  floatingPositionPixels,
  moveFloatingPlacement,
  type FloatingAnchor,
  type FloatingPlacement,
  type FloatingSize,
} from '@/features/custom-ui/floating-placement'
import { useFloatingPlacementStore } from '@/features/custom-ui/floating-placement-store'
import { ComponentHost } from '@/features/custom-ui/native/ComponentHost'
import type { PlacedComponent } from '@/features/custom-ui/placements'
import { customUiQueryKeys } from '@/features/custom-ui/query-keys'
import { useComponentFix } from '@/features/custom-ui/studio/use-component-agent'

type DragState = {
  pointerId: number
  element: HTMLButtonElement
  x: number
  y: number
  initial: FloatingPlacement
  latest: FloatingPlacement
}

const EMPTY_SIZE = { width: 0, height: 0 }
// 旧版浮窗按组件 id 记位置（desktop:<componentId>）；实例 id 是 UUID v4。非 UUID 的键是旧专注岛 / iframe 组件留下的孤儿。
const INSTANCE_NODE =
  /^(?:desktop|mobile):[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

// 拖动 / 复位 / 关闭小键：实心键帽面 + 键帽描边与投影，不用半透明遮罩；不向外扩命中区，避免抢走紧贴其旁的组件内容点击。
const FLOATING_KEY =
  'flex items-center justify-center rounded-full bg-key text-content-muted shadow-key ring-1 ring-key-border hover:text-content'

function floatingSize(placed: PlacedComponent) {
  const manifest = placed.component.manifest
  const size = (manifest && manifestPlacement(manifest, 'floating')?.size) || FLOATING_DEFAULT_SIZE
  return { width: size.w, height: size.h }
}

function FloatingComponent({
  placed,
  boundaryRef,
  anchorRef,
  notify,
  onClose,
  stackIndex,
}: {
  placed: PlacedComponent
  boundaryRef: RefObject<HTMLDivElement | null>
  anchorRef?: RefObject<HTMLElement | null>
  notify: (message: string) => void
  onClose: (instanceId: string) => void
  stackIndex: number
}) {
  const { t } = useI18n()
  const { instance, component } = placed
  const fix = useComponentFix(component, notify)
  const key = floatingPlacementKey(instance.instanceId, false)
  const saved = useFloatingPlacementStore(
    (state) => state.positions.find((item) => item.key === key)?.position,
  )
  const elementRef = useRef<HTMLDivElement>(null)
  const dragRef = useRef<DragState | null>(null)
  const [draft, setDraft] = useState<FloatingPlacement | null>(null)
  const [sizes, setSizes] = useState<{
    bounds: FloatingSize
    widget: FloatingSize
    anchor?: FloatingAnchor
  }>({ bounds: EMPTY_SIZE, widget: EMPTY_SIZE })
  const position =
    saved ?? defaultFloatingPlacement(sizes.bounds, sizes.widget, sizes.anchor, stackIndex + 1)
  const helpId = useId()
  const size = floatingSize(placed)
  const label = instance.title || component.manifest?.name || component.name

  useEffect(() => {
    const boundary = boundaryRef.current
    const element = elementRef.current
    if (!boundary || !element) return
    let observedAnchor: HTMLElement | null = null
    const observer = new ResizeObserver(() => measure())
    const measure = () => {
      const anchor = anchorRef?.current
      if (anchor !== observedAnchor) {
        if (observedAnchor) observer.unobserve(observedAnchor)
        observedAnchor = anchor ?? null
        if (anchor) observer.observe(anchor)
      }
      const boundaryRect = boundary.getBoundingClientRect()
      const anchorRect = anchor?.getBoundingClientRect()
      const next = {
        bounds: { width: boundary.clientWidth, height: boundary.clientHeight },
        widget: { width: element.offsetWidth, height: element.offsetHeight },
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
        current.widget.width === next.widget.width &&
        current.widget.height === next.widget.height &&
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
    window.addEventListener('resize', measure)
    return () => {
      observer.disconnect()
      window.removeEventListener('resize', measure)
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
      .setPosition(key, moveFloatingPlacement(position, delta, sizes.bounds, sizes.widget))
  }
  const pixels = floatingPositionPixels(draft ?? position, sizes.bounds, sizes.widget)
  return (
    <div
      ref={elementRef}
      role="group"
      aria-label={label}
      data-floating-widget={instance.instanceId}
      data-floating-component={component.id}
      className="pointer-events-auto absolute flex max-w-full flex-col px-6"
      style={{
        left: pixels.x,
        top: pixels.y,
        width: size.width + 48,
        maxHeight: '100%',
        visibility: sizes.bounds.width > 0 ? 'visible' : 'hidden',
      }}
    >
      <div className="absolute top-1/2 left-0 z-10 flex -translate-y-1/2 flex-col items-center gap-1">
        <button
          type="button"
          aria-label={t('custom-ui:floating.move')}
          aria-describedby={helpId}
          title={t('custom-ui:floating.moveHint')}
          className={cn(FLOATING_KEY, 'h-6 w-5 touch-none')}
          style={{ cursor: draft ? 'grabbing' : 'grab' }}
          data-floating-move
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
              sizes.widget,
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
          className={cn(FLOATING_KEY, 'size-5')}
        >
          <RotateCcw className="size-3" />
        </button>
        <span id={helpId} className="sr-only">
          {t('custom-ui:floating.moveHint')}
        </span>
      </div>
      <button
        type="button"
        aria-label={t('custom-ui:floating.close')}
        title={t('custom-ui:floating.close')}
        data-floating-close
        onClick={() => onClose(instance.instanceId)}
        className={cn(FLOATING_KEY, 'absolute top-1/2 right-0 z-10 size-5 -translate-y-1/2')}
      >
        <X className="size-3" />
      </button>
      <div
        className="flex min-h-0 flex-col overflow-hidden rounded-md border border-stroke bg-panel shadow-key"
        style={{ height: size.height }}
      >
        <ComponentHost
          component={component}
          instance={{
            instanceId: instance.instanceId,
            surface: 'floating',
            title: instance.title,
            props: instance.props,
          }}
          notify={notify}
          onRequestFix={fix}
          className="h-full"
        />
      </div>
    </div>
  )
}

export function FloatingComponents({
  placed,
  notify,
  anchorRef,
}: {
  placed: readonly PlacedComponent[]
  notify: (message: string) => void
  anchorRef?: RefObject<HTMLElement | null>
}) {
  const { t } = useI18n()
  const client = useQueryClient()
  const boundaryRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    // 清掉旧版按组件 id 记下的浮窗位置（负责人决定：之前的组件都删除，偏好里的孤儿键一并清掉）。
    const store = useFloatingPlacementStore.getState()
    for (const entry of store.positions)
      if (!INSTANCE_NODE.test(entry.key)) store.resetPosition(entry.key)
  }, [])
  const close = (instanceId: string) => {
    updateComponentInstance(instanceId, { enabled: false }).then(
      () => client.invalidateQueries({ queryKey: customUiQueryKeys.instances() }),
      () => notify(t('custom-ui:floating.closeFailed')),
    )
  }
  return (
    <div
      ref={boundaryRef}
      className="pointer-events-none fixed inset-x-2 top-12 bottom-2 z-40 overflow-hidden"
      data-floating-custom-ui
    >
      {placed.map((entry, index) => (
        <FloatingComponent
          key={entry.instance.instanceId}
          placed={entry}
          boundaryRef={boundaryRef}
          anchorRef={anchorRef}
          notify={notify}
          onClose={close}
          stackIndex={index}
        />
      ))}
    </div>
  )
}
