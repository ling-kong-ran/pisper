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
      const narrowViewport = window.innerWidth <= 650
      const availableWidth =
        element.getBoundingClientRect().width - (narrowViewport ? 44 : TOOL_TRIGGER_WIDTH)
      setCapacity(Math.max(0, Math.floor(availableWidth / (narrowViewport ? 48 : TOOL_SLOT_WIDTH))))
    }

    update()
    // 窗口跨过触控断点时，容器宽度可能不变，但按钮会从 36px 变成 44px。
    window.addEventListener('resize', update)
    if (typeof ResizeObserver === 'undefined') {
      return () => window.removeEventListener('resize', update)
    }

    const observer = new ResizeObserver(update)
    observer.observe(element)
    return () => {
      observer.disconnect()
      window.removeEventListener('resize', update)
    }
  }, [ref])

  return capacity
}
