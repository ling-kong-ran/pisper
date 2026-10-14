// 菜单定位 hook：打开时把悬浮菜单约束在视口内（留出边距），
// 防止菜单溢出被裁切。
import { useLayoutEffect, type RefObject } from 'react'

const VIEWPORT_GUTTER = 8

export function useViewportMenuOffset(open: boolean, menuRef: RefObject<HTMLElement | null>) {
  useLayoutEffect(() => {
    if (!open) return undefined

    const position = () => {
      const menu = menuRef.current
      if (!menu) return
      menu.style.setProperty('--menu-x-offset', '0px')
      menu.style.setProperty('--menu-y-offset', '0px')
      menu.style.removeProperty('max-height')
      menu.style.removeProperty('overflow-y')

      let visibleLeft = VIEWPORT_GUTTER
      let visibleTop = VIEWPORT_GUTTER
      let visibleRight = window.innerWidth - VIEWPORT_GUTTER
      let visibleBottom = window.innerHeight - VIEWPORT_GUTTER
      let ancestor = menu.parentElement
      while (ancestor) {
        const style = getComputedStyle(ancestor)
        if (/(auto|hidden|scroll|clip)/.test(`${style.overflowX} ${style.overflowY}`)) {
          const bounds = ancestor.getBoundingClientRect()
          visibleLeft = Math.max(visibleLeft, bounds.left + VIEWPORT_GUTTER)
          visibleTop = Math.max(visibleTop, bounds.top + VIEWPORT_GUTTER)
          visibleRight = Math.min(visibleRight, bounds.right - VIEWPORT_GUTTER)
          visibleBottom = Math.min(visibleBottom, bounds.bottom - VIEWPORT_GUTTER)
        }
        ancestor = ancestor.parentElement
      }

      const availableHeight = Math.max(120, visibleBottom - visibleTop)
      let bounds = menu.getBoundingClientRect()
      if (bounds.height > availableHeight) {
        menu.style.maxHeight = `${availableHeight}px`
        menu.style.overflowY = 'auto'
        bounds = menu.getBoundingClientRect()
      }
      const xOffset =
        bounds.right > visibleRight
          ? visibleRight - bounds.right
          : bounds.left < visibleLeft
            ? visibleLeft - bounds.left
            : 0
      const yOffset =
        bounds.bottom > visibleBottom
          ? visibleBottom - bounds.bottom
          : bounds.top < visibleTop
            ? visibleTop - bounds.top
            : 0
      menu.style.setProperty('--menu-x-offset', `${xOffset}px`)
      menu.style.setProperty('--menu-y-offset', `${yOffset}px`)
    }

    position()
    window.addEventListener('resize', position)
    return () => window.removeEventListener('resize', position)
  }, [menuRef, open])
}
