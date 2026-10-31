// 整页的“Esc 返回”（目前只有设置整页，规格 2.3 侧栏上写明“← 返回 Esc”）：Esc 永远不用于离开页面的唯一例外。
// 只在这几种情况都不成立时离开：按键已被别人处理（返回层、弹层、输入框自己的 Esc）、输入法组合中、
// 带修饰键、焦点在可编辑元素里、按下时有打开着的弹层（弹层会在同一次按键里自己关掉，捕获阶段先记下）。
// 监听登记在 BackNavigation 之后：有返回层时它先处理并 preventDefault，这里就不再离开。
import { useEffect, useRef } from 'react'
import { hasOpenOverlay } from '@/lib/back-navigation'
import { isEditableTarget } from '@/lib/shortcuts'

export function useEscapeLeave(onLeave: () => void, enabled = true) {
  const leaveRef = useRef(onLeave)
  leaveRef.current = onLeave
  useEffect(() => {
    if (!enabled) return
    const overlayEvents = new WeakSet<Event>()
    const capture = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && hasOpenOverlay()) overlayEvents.add(event)
    }
    const bubble = (event: KeyboardEvent) => {
      if (
        event.key !== 'Escape' ||
        event.defaultPrevented ||
        event.isComposing ||
        event.altKey ||
        event.ctrlKey ||
        event.metaKey ||
        event.shiftKey ||
        overlayEvents.has(event) ||
        isEditableTarget(event.target)
      )
        return
      event.preventDefault()
      leaveRef.current()
    }
    window.addEventListener('keydown', capture, true)
    window.addEventListener('keydown', bubble)
    return () => {
      window.removeEventListener('keydown', capture, true)
      window.removeEventListener('keydown', bubble)
    }
  }, [enabled])
}
