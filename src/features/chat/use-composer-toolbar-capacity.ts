// 根据工具区实际宽度计算可常驻的按钮数量，分屏和窗口缩放时实时回退到收纳区。
import { useLayoutEffect, useState, type RefObject } from 'react'

const TOOL_TRIGGER_WIDTH = 36
const TOOL_SLOT_WIDTH = 40

export function useComposerToolbarCapacity(ref: RefObject<HTMLElement | null>) {
  const [capacity, setCapacity] = useState(0)

  useLayoutEffect(() => {
    const element = ref.current
    if (!element) return undefined

    const update = () => {
      const triggerWidth =
        element.querySelector('.composer-tools-trigger')?.getBoundingClientRect().width ||
        TOOL_TRIGGER_WIDTH
      const availableWidth = element.getBoundingClientRect().width - triggerWidth - 4
      setCapacity(Math.max(0, availableWidth / TOOL_SLOT_WIDTH))
    }

    update()
    let frame: number | null = null
    // 按钮收纳会改变被观察区域的布局，不能在 ResizeObserver 交付期间同步重排。
    const scheduleUpdate = () => {
      if (frame !== null) return
      frame = requestAnimationFrame(() => {
        frame = null
        update()
      })
    }
    // 窗口跨过触控断点时，容器宽度可能不变，但按钮会从 36px 变成 44px。
    window.addEventListener('resize', scheduleUpdate)
    if (typeof ResizeObserver === 'undefined') {
      return () => {
        window.removeEventListener('resize', scheduleUpdate)
        if (frame !== null) cancelAnimationFrame(frame)
      }
    }

    const observer = new ResizeObserver(scheduleUpdate)
    observer.observe(element)
    return () => {
      observer.disconnect()
      window.removeEventListener('resize', scheduleUpdate)
      if (frame !== null) cancelAnimationFrame(frame)
    }
  }, [ref])

  return capacity
}
