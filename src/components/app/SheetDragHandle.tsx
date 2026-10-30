// 底部表单顶部的拖拽条：向下拖过阈值（或快速下滑）就关闭表单，不够时弹回原位。
// 只在拖动时给表单本体写 transform（不触发布局），松手后清掉；关闭仍走表单自己的 onOpenChange，
// 所以 Esc、Android 返回键与关闭按钮的行为不变。拖拽条本身不可聚焦：键盘与读屏用户用表单里的关闭键。
// 拖过阈值关闭时表单停在拖到的位置，退场动画从这里继续往下滑出（不先弹回原位再退场）。
import { useRef, type PointerEvent } from 'react'
import { cn } from '@/lib/utils'

const DISMISS_DISTANCE = 96
const DISMISS_VELOCITY = 0.6
// 退场时在拖到的位置上再往下滑的距离：与表单退场的 slide-out-to-bottom-10 相同（表单自身高度的 10%）。
const EXIT_EXTRA_DISTANCE = '10%'

type DragState = {
  pointerId: number
  startY: number
  startAt: number
  sheet: HTMLElement
  offset: number
}

export function SheetDragHandle({
  onDismiss,
  className,
}: {
  onDismiss: () => void
  className?: string
}) {
  const drag = useRef<DragState | null>(null)
  const reset = (state: DragState) => {
    state.sheet.style.transform = ''
    state.sheet.style.transition = ''
  }
  const start = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    const sheet = event.currentTarget.closest<HTMLElement>('[data-slot="sheet-content"]')
    if (!sheet) return
    event.currentTarget.setPointerCapture?.(event.pointerId)
    // 拖动期间跟手，不叠加表单自身的过渡。
    sheet.style.transition = 'none'
    drag.current = {
      pointerId: event.pointerId,
      startY: event.clientY,
      startAt: performance.now(),
      sheet,
      offset: 0,
    }
  }
  const move = (event: PointerEvent<HTMLDivElement>) => {
    const state = drag.current
    if (!state || state.pointerId !== event.pointerId) return
    state.offset = Math.max(0, event.clientY - state.startY)
    state.sheet.style.transform = `translateY(${state.offset}px)`
  }
  // 关闭：保留拖到的 transform 作为退场动画的起点（tw-animate 的 exit 关键帧只写终点），
  // 终点改成在拖到的位置上再往下滑一段（--tw-exit-translate-y）。表单卸载时内联样式随之消失；
  // 表单没有关上（宿主拒绝），或退场途中又被打开（进场动画开始）时清掉，回到原位。
  const dismissFromOffset = (state: DragState) => {
    const { sheet } = state
    sheet.style.setProperty(
      '--tw-exit-translate-y',
      `calc(${state.offset}px + ${EXIT_EXTRA_DISTANCE})`,
    )
    const restore = () => {
      sheet.removeEventListener('animationstart', reopened)
      reset(state)
      sheet.style.removeProperty('--tw-exit-translate-y')
    }
    const reopened = (event: AnimationEvent) => {
      if (event.target === sheet && sheet.dataset.state !== 'closed') restore()
    }
    sheet.addEventListener('animationstart', reopened)
    onDismiss()
    requestAnimationFrame(() => {
      if (sheet.isConnected && sheet.dataset.state !== 'closed') restore()
    })
  }
  const end = (event: PointerEvent<HTMLDivElement>) => {
    const state = drag.current
    if (!state || state.pointerId !== event.pointerId) return
    drag.current = null
    const velocity = state.offset / Math.max(1, performance.now() - state.startAt)
    if (state.offset >= DISMISS_DISTANCE || (state.offset > 24 && velocity >= DISMISS_VELOCITY)) {
      dismissFromOffset(state)
      return
    }
    reset(state)
  }
  const cancel = () => {
    const state = drag.current
    drag.current = null
    if (state) reset(state)
  }
  return (
    <div
      aria-hidden="true"
      data-sheet-drag-handle
      className={cn(
        // 可拖区域整行宽、32px 高，视觉上只有一根 36×4 的短条。
        'flex h-8 w-full flex-none cursor-grab touch-none items-start justify-center select-none active:cursor-grabbing',
        className,
      )}
      onPointerDown={start}
      onPointerMove={move}
      onPointerUp={end}
      onPointerCancel={cancel}
      onLostPointerCapture={cancel}
    >
      <span className="mt-2.5 h-1 w-9 rounded-full bg-stroke-strong" />
    </div>
  )
}
