// 输入框收纳区：通过 portal 避开分屏面板裁切，只承载主动或响应式收纳的工具。
import type { ReactNode, RefObject } from 'react'
import { AnchoredPopupMenu } from '@/features/chat/AnchoredPopupMenu'

export function ComposerToolTray({
  open,
  children,
  label,
  trayId,
  anchorRef,
  menuRef,
  placement = 'top',
}: {
  open: boolean
  children: ReactNode
  label: string
  trayId: string
  anchorRef: RefObject<HTMLElement | null>
  menuRef: RefObject<HTMLDivElement | null>
  placement?: 'top' | 'bottom'
}) {
  return (
    <AnchoredPopupMenu
      open={open}
      anchorRef={anchorRef}
      menuRef={menuRef}
      placement={placement}
      align="start"
      className="composer-tool-tray-shell w-[272px] max-w-[calc(100vw-16px)] overflow-visible rounded-[var(--r-md)] border border-[var(--stroke)] bg-[var(--solid)] p-1.5 shadow-[0_18px_42px_-18px_var(--menu-shadow)] motion-safe:animate-in motion-safe:fade-in-0 data-[side=top]:motion-safe:slide-in-from-bottom-1 data-[side=bottom]:motion-safe:slide-in-from-top-1"
      role="toolbar"
      ariaLabel={label}
    >
      <div
        id={trayId}
        className="composer-tool-tray flex max-h-[min(52vh,360px)] min-h-9 w-full min-w-0 flex-wrap items-center gap-1 overflow-x-hidden overflow-y-auto overscroll-contain"
      >
        {children}
      </div>
    </AnchoredPopupMenu>
  )
}
