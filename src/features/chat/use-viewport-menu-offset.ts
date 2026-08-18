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
      const bounds = menu.getBoundingClientRect()
      const offset =
        bounds.right > window.innerWidth - VIEWPORT_GUTTER
          ? window.innerWidth - VIEWPORT_GUTTER - bounds.right
          : bounds.left < VIEWPORT_GUTTER
            ? VIEWPORT_GUTTER - bounds.left
            : 0
      menu.style.setProperty('--menu-x-offset', `${offset}px`)
    }

    position()
    window.addEventListener('resize', position)
    return () => window.removeEventListener('resize', position)
  }, [menuRef, open])
}
