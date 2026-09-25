// 锚定弹层菜单：通过 portal 挂载到 body，用 fixed 定位吸附在触发按钮上方/下方，
// 并按视口边界平移与限高。收纳区（composer-tool-tray）这类 overflow 滚动容器
// 会裁切内部 absolute 定位的菜单，portal 后彻底绕开裁切问题。
import { useEffect, useLayoutEffect, type ReactNode, type RefObject } from 'react'
import { createPortal } from 'react-dom'
import { resolveAnchoredPopupLayout } from './anchored-popup-layout'

type AnchoredPopupMenuProps = {
  open: boolean
  // 触发按钮容器：菜单以它为锚点定位。
  anchorRef: RefObject<HTMLElement | null>
  menuRef: RefObject<HTMLDivElement | null>
  // top 在锚点上方展开（输入框/收纳区场景），bottom 在下方展开（如空会话头部）。
  placement?: 'top' | 'bottom'
  // end 时菜单右缘对齐锚点右缘，默认左缘对齐。
  align?: 'start' | 'end'
  matchAnchorWidth?: boolean
  maxHeight?: number
  id?: string
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
  matchAnchorWidth = false,
  maxHeight,
  id,
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
      const viewport = window.visualViewport
      // 软键盘会缩小可视视口；上置输入区优先下展，两边都不足时按实际空间滚动。
      menu.style.width = matchAnchorWidth ? `${anchorBounds.width}px` : ''
      const layout = resolveAnchoredPopupLayout({
        anchor: anchorBounds,
        menu: { width: menu.getBoundingClientRect().width, height: menu.scrollHeight },
        viewport: {
          left: viewport?.offsetLeft ?? 0,
          top: viewport?.offsetTop ?? 0,
          width: viewport?.width ?? window.innerWidth,
          height: viewport?.height ?? window.innerHeight,
        },
        placement,
        align,
        maxHeight,
      })
      menu.dataset.side = layout.side
      menu.style.maxHeight = `${layout.maxHeight}px`
      menu.style.overflowY = 'auto'
      menu.style.width = `${layout.width}px`
      menu.style.top = `${layout.top}px`
      menu.style.bottom = 'auto'
      menu.style.right = 'auto'
      menu.style.left = `${layout.left}px`
    }
    position()
    let frame: number | null = null
    const schedulePosition = () => {
      if (frame !== null) return
      frame = window.requestAnimationFrame(() => {
        frame = null
        position()
      })
    }
    const observer =
      typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(schedulePosition)
    if (menuRef.current) observer?.observe(menuRef.current)
    if (anchorRef.current) observer?.observe(anchorRef.current)
    window.addEventListener('resize', position)
    // 捕获阶段监听滚动：收纳区或页面滚动时让菜单跟随锚点，避免悬浮脱节。
    window.addEventListener('scroll', position, true)
    window.visualViewport?.addEventListener('resize', schedulePosition)
    window.visualViewport?.addEventListener('scroll', schedulePosition)
    return () => {
      observer?.disconnect()
      if (frame !== null) window.cancelAnimationFrame(frame)
      window.removeEventListener('resize', position)
      window.removeEventListener('scroll', position, true)
      window.visualViewport?.removeEventListener('resize', schedulePosition)
      window.visualViewport?.removeEventListener('scroll', schedulePosition)
    }
  }, [open, anchorRef, menuRef, placement, align, matchAnchorWidth, maxHeight])

  if (!open) return null
  return createPortal(
    <div
      ref={menuRef}
      id={id}
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
