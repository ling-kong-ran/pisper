import { useEffect, useRef } from 'react'
import { appBackStack } from '@/lib/back-navigation'

function hasOverlay() {
  return [
    ...document.querySelectorAll<HTMLElement>(
      '[role="dialog"], [role="alertdialog"], [role="menu"], [data-slot="popover-content"]',
    ),
  ].some(
    (element) =>
      element.dataset.state !== 'closed' &&
      element.getAttribute('aria-hidden') !== 'true' &&
      element.getClientRects().length > 0,
  )
}

// 原生返回通过同一个 Escape 路径委派给现有弹层，未消费时再交给路由层的上级页面
// （见 shell-tabs.shellBackPath：会话主页是根，点进来的会话回来处，一级页回主页，更多的子页回更多），
// 都不处理才交回平台。
// 路由层返回只挂在原生返回上：键盘 Esc 永远不用于离开页面。
export function BackNavigation({ onRouteBack }: { onRouteBack?: () => boolean }) {
  const routeBackRef = useRef(onRouteBack)
  routeBackRef.current = onRouteBack
  useEffect(() => {
    const overlayEvents = new WeakSet<Event>()
    const capture = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && hasOverlay()) overlayEvents.add(event)
    }
    const bubble = (event: KeyboardEvent) => {
      if (
        event.key !== 'Escape' ||
        event.defaultPrevented ||
        event.isComposing ||
        overlayEvents.has(event)
      )
        return
      if (appBackStack.dismiss()) event.preventDefault()
    }
    const request = () => {
      const overlay = hasOverlay()
      const event = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
      ;(document.activeElement || document.body).dispatchEvent(event)
      if (overlay || event.defaultPrevented) return true
      return routeBackRef.current?.() ?? false
    }
    const host = window as Window & { __pisperRequestBack?: () => boolean }
    host.__pisperRequestBack = request
    window.addEventListener('keydown', capture, true)
    window.addEventListener('keydown', bubble)
    return () => {
      if (host.__pisperRequestBack === request) delete host.__pisperRequestBack
      window.removeEventListener('keydown', capture, true)
      window.removeEventListener('keydown', bubble)
    }
  }, [])
  return null
}
