import { useLayoutEffect, useRef, useState } from 'react'
import { SESSION_NAVIGATION_WILL_CHANGE, TASK_VIEW_WILL_CHANGE } from './events'

export type ResourceReadingPosition = { top: number; left: number }
export function useResourceReading<T extends HTMLElement>(
  position: ResourceReadingPosition | undefined,
  onChange: (position: ResourceReadingPosition) => void,
  identity = '',
) {
  const [element, ref] = useState<T | null>(null)
  const snapshot = useRef({ identity, position: position || { top: 0, left: 0 } })
  snapshot.current = { identity, position: position || { top: 0, left: 0 } }
  const latest = useRef(onChange)
  latest.current = onChange
  useLayoutEffect(() => {
    if (!element) return
    const target = snapshot.current.position
    let pending = true
    let frame: number | null = null
    const save = () => {
      if (
        snapshot.current.identity === identity &&
        !pending &&
        element.isConnected &&
        element.clientHeight > 0
      )
        latest.current({ top: element.scrollTop, left: element.scrollLeft })
    }
    const restore = () => {
      frame = null
      if (!pending) return
      element.scrollTo({
        top: target.top,
        left: target.left,
        behavior: 'instant',
      })
      if (
        Math.abs(element.scrollTop - target.top) < 1 &&
        Math.abs(element.scrollLeft - target.left) < 1
      )
        pending = false
    }
    const schedule = () => {
      if (pending && frame === null) frame = requestAnimationFrame(restore)
    }
    const userIntent = () => {
      pending = false
      save()
    }
    // 懒加载差异及虚拟行完成测量前保持目标；用户开始操作后立即交还滚动控制。
    restore()
    const observer = new ResizeObserver(schedule)
    observer.observe(element)
    for (const child of element.children) observer.observe(child)
    element.addEventListener('scroll', save)
    element.addEventListener('wheel', userIntent, { passive: true })
    element.addEventListener('pointerdown', userIntent)
    element.addEventListener('keydown', userIntent)
    window.addEventListener(SESSION_NAVIGATION_WILL_CHANGE, save)
    window.addEventListener(TASK_VIEW_WILL_CHANGE, save)
    return () => {
      save()
      observer.disconnect()
      if (frame !== null) cancelAnimationFrame(frame)
      element.removeEventListener('scroll', save)
      element.removeEventListener('wheel', userIntent)
      element.removeEventListener('pointerdown', userIntent)
      element.removeEventListener('keydown', userIntent)
      window.removeEventListener(SESSION_NAVIGATION_WILL_CHANGE, save)
      window.removeEventListener(TASK_VIEW_WILL_CHANGE, save)
    }
  }, [element, identity])
  return { element, ref }
}
