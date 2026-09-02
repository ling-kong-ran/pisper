// 锚定弹层菜单：通过 portal 挂载到 body，用 fixed 定位吸附在触发按钮上方/下方，
// 并按视口边界平移与限高。收纳区（composer-tool-tray）这类 overflow 滚动容器
// 会裁切内部 absolute 定位的菜单，portal 后彻底绕开裁切问题。
import { useEffect, useLayoutEffect, type ReactNode, type RefObject } from 'react'
import { createPortal } from 'react-dom'

const VIEWPORT_GUTTER = 8
const MENU_GAP = 8

type AnchoredPopupMenuProps = {
  open: boolean
  // 触发按钮容器：菜单以它为锚点定位。
  anchorRef: RefObject<HTMLElement | null>
  menuRef: RefObject<HTMLDivElement | null>
  // top 在锚点上方展开（输入框/收纳区场景），bottom 在下方展开（如空会话头部）。
  placement?: 'top' | 'bottom'
  // end 时菜单右缘对齐锚点右缘，默认左缘对齐。
  align?: 'start' | 'end'
  className?: string
  role?: string
  ariaLabel?: string
  // 打开期间按 Esc 时调用：调用方在其中关闭弹层并把焦点归还给触发元素。
  onClose?: () => void
  children: ReactNode
}

export function AnchoredPopupMenu({
  open,
  anchorRef,
  menuRef,
  placement = 'top',
  align = 'start',
  className,
  role,
  ariaLabel,
  onClose,
  children,
}: AnchoredPopupMenuProps) {
  // Esc 关闭：浮层 portal 到 body 后没有原生对话框的焦点管理，
  // 这里统一拦截 Escape 并交给调用方关闭 + 把焦点归还触发元素。
  useEffect(() => {
    if (!open || !onClose) return undefined
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.stopPropagation()
      onClose()
    }
    document.addEventListener('keydown', handleEscape)
    return () => document.removeEventListener('keydown', handleEscape)
  }, [open, onClose])

  useLayoutEffect(() => {
    if (!open) return undefined
    const position = () => {
      const menu = menuRef.current
      const anchor = anchorRef.current
      if (!menu || !anchor) return
      const anchorBounds = anchor.getBoundingClientRect()
      const visibleRight = window.innerWidth - VIEWPORT_GUTTER
      const visibleBottom = window.innerHeight - VIEWPORT_GUTTER

      // 垂直方向可用空间受锚点位置限制，超出则限高并让菜单自身滚动。
      const availableHeight = Math.max(
        120,
        placement === 'top'
          ? anchorBounds.top - MENU_GAP - VIEWPORT_GUTTER
          : visibleBottom - anchorBounds.bottom - MENU_GAP,
      )
      menu.style.maxHeight = `${availableHeight}px`
      menu.style.overflowY = 'auto'
      if (placement === 'top') {
        menu.style.top = 'auto'
        menu.style.bottom = `${window.innerHeight - anchorBounds.top + MENU_GAP}px`
      } else {
        menu.style.bottom = 'auto'
        menu.style.top = `${anchorBounds.bottom + MENU_GAP}px`
      }
      if (align === 'end') {
        menu.style.left = 'auto'
        menu.style.right = `${window.innerWidth - anchorBounds.right}px`
      } else {
        menu.style.right = 'auto'
        menu.style.left = `${anchorBounds.left}px`
      }

      // 水平越界时夹回视口。
      const bounds = menu.getBoundingClientRect()
      if (bounds.right > visibleRight) {
        menu.style.right = 'auto'
        menu.style.left = `${Math.max(VIEWPORT_GUTTER, visibleRight - bounds.width)}px`
      } else if (bounds.left < VIEWPORT_GUTTER) {
        menu.style.right = 'auto'
        menu.style.left = `${VIEWPORT_GUTTER}px`
      }
    }
    position()
    window.addEventListener('resize', position)
    // 捕获阶段监听滚动：收纳区或页面滚动时让菜单跟随锚点，避免悬浮脱节。
    window.addEventListener('scroll', position, true)
    return () => {
      window.removeEventListener('resize', position)
      window.removeEventListener('scroll', position, true)
    }
  }, [open, anchorRef, menuRef, placement, align])

  if (!open) return null
  return createPortal(
    <div
      ref={menuRef}
      className={className}
      style={{ position: 'fixed', zIndex: 65 }}
      role={role}
      aria-label={ariaLabel}
    >
      {children}
    </div>,
    document.body,
  )
}
