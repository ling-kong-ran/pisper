// 关闭后只为退场动画多挂载一会儿的“在场”状态（例如手机底部表单）：
// - open 变为 false 的同一次渲染就进入退场（leaving），宿主据此继续渲染关闭态（data-state=closed）播放退场动画，
//   不会先卸载再重挂，也没有空白帧；
// - 宿主在退场动画结束（animationend / transitionend）时调用 finish 卸载；漏掉结束事件（标签页隐藏、动画被取消、
//   元素提前 display:none）时由兜底超时卸载，关闭后最多再挂 fallbackMs；
// - 减少动效（界面设置或系统偏好）时不播退场，在绘制前直接卸载；
// - 退场途中重新打开立即取消退场，同一个实例继续使用（不重挂、不丢状态）。
import { useCallback, useLayoutEffect, useState } from 'react'
import { motionReduced } from '@/lib/motion'

export type ExitPresence = { open: boolean; leaving: boolean }

// 兜底时长：底部表单退场是 --d-base（180ms），留出一倍多的余量。
export const EXIT_PRESENCE_FALLBACK_MS = 400

// 按新的 open 推进：打开（含退场途中重新打开）取消退场，关闭进入退场；open 没变时原样返回。
export function nextExitPresence(state: ExitPresence, open: boolean): ExitPresence {
  if (state.open === open) return state
  return { open, leaving: !open }
}

// 退场结束（动画结束、兜底超时或减少动效）：只结束仍在进行的退场，重新打开后迟到的事件不起作用。
export function finishExitPresence(state: ExitPresence): ExitPresence {
  return state.leaving ? { open: state.open, leaving: false } : state
}

// 是否还要渲染：打开着，或关闭后的退场还没结束。关闭且退场结束 → 不渲染。
export function exitPresenceMounted(state: ExitPresence) {
  return state.open || state.leaving
}

export function useExitPresence(open: boolean, fallbackMs = EXIT_PRESENCE_FALLBACK_MS) {
  const [state, setState] = useState<ExitPresence>(() => ({ open, leaving: false }))
  // 渲染期间按上一次记下的 open 推进（React 的“随 props 调整 state”写法）：关闭的这一帧就已在退场中。
  let current = state
  if (state.open !== open) {
    current = nextExitPresence(state, open)
    setState(current)
  }
  const leaving = current.leaving
  const finish = useCallback(() => setState(finishExitPresence), [])
  useLayoutEffect(() => {
    if (!leaving) return
    if (motionReduced()) {
      finish()
      return
    }
    const timer = window.setTimeout(finish, fallbackMs)
    return () => window.clearTimeout(timer)
  }, [fallbackMs, finish, leaving])
  return { mounted: exitPresenceMounted(current), leaving, finish }
}
