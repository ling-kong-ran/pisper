// 根据工具区实际宽度计算可常驻的按钮数量，分屏和窗口缩放时实时回退到收纳区。
import { useLayoutEffect, useState, type RefObject } from 'react'

const TOOL_TRIGGER_WIDTH = 36
const TOOL_SLOT_WIDTH = 40

export function useComposerToolbarCapacity(ref: RefObject<HTMLElement | null>) {
  const [capacity, setCapacity] = useState(Number.POSITIVE_INFINITY)

  useLayoutEffect(() => {
    const element = ref.current
    if (!element) return undefined

    const update = () => {
      const availableWidth = element.getBoundingClientRect().width - TOOL_TRIGGER_WIDTH
      setCapacity(Math.max(0, Math.floor(availableWidth / TOOL_SLOT_WIDTH)))
    }

    update()
    if (typeof ResizeObserver === 'undefined') {
      window.addEventListener('resize', update)
      return () => window.removeEventListener('resize', update)
    }

    const observer = new ResizeObserver(update)
    observer.observe(element)
    return () => observer.disconnect()
  }, [ref])

  return capacity
}
