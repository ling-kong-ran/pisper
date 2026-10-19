import { useEffect } from 'react'
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

// 原生返回通过同一个 Escape 路径委派给现有弹层，未消费时才交回平台。
export function BackNavigation() {
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
      return overlay || event.defaultPrevented
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
