import { useState, type ReactNode } from 'react'
import type {
  BuiltInContextMenuItem,
  DockviewApi,
  GetTabContextMenuItemsParams,
  ReactContextMenuItemConfig,
} from 'dockview-react'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'

type MenuItem = BuiltInContextMenuItem | ReactContextMenuItemConfig

type ChatDockTabContextMenuProps = {
  api: DockviewApi | null
  getItems: (params: Pick<GetTabContextMenuItemsParams, 'panel'>) => MenuItem[]
  children: ReactNode
}

type OpenTabMenu = {
  open: boolean
  origin: HTMLElement
  panelId: string
  x: number
  y: number
  items: MenuItem[]
}

export function ChatDockTabContextMenu({ api, getItems, children }: ChatDockTabContextMenuProps) {
  const [menu, setMenu] = useState<OpenTabMenu | null>(null)

  return (
    <DropdownMenu
      open={menu?.open ?? false}
      onOpenChange={(open) => setMenu((current) => (current ? { ...current, open } : null))}
    >
      <div
        className="contents"
        onKeyDownCapture={(event) => {
          const target = event.target
          if (
            event.defaultPrevented ||
            !(target instanceof HTMLElement) ||
            !target.matches('.dv-tab[role="tab"]') ||
            !(event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10'))
          )
            return
          const panel = api?.panels.find((candidate) => target.contains(candidate.view.tab.element))
          if (!panel) return
          // Dockview 的鼠标菜单没有键盘导航；复用同一动作列表交给 Radix 管理焦点。
          event.preventDefault()
          event.stopPropagation()
          const rect = target.getBoundingClientRect()
          setMenu({
            open: true,
            origin: target,
            panelId: panel.id,
            x: rect.left,
            y: rect.bottom,
            items: getItems({ panel }),
          })
        }}
      >
        {children}
        <DropdownMenuTrigger asChild>
          <span
            aria-hidden="true"
            tabIndex={-1}
            className="pointer-events-none fixed size-0"
            style={{ left: menu?.x ?? 0, top: menu?.y ?? 0 }}
          />
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="start"
          sideOffset={0}
          className="w-auto"
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            if (!menu) return
            const panel = api?.getPanel(menu.panelId) ?? api?.activePanel
            const tab = menu.origin.isConnected
              ? menu.origin
              : panel?.view.tab.element.closest<HTMLElement>('.dv-tab')
            tab?.focus()
          }}
        >
          {menu?.items.map((item, index) =>
            item === 'separator' ? (
              <DropdownMenuSeparator key={index} />
            ) : typeof item === 'object' ? (
              <DropdownMenuItem key={item.label} disabled={item.disabled} onSelect={item.action}>
                {item.label}
              </DropdownMenuItem>
            ) : null,
          )}
        </DropdownMenuContent>
      </div>
    </DropdownMenu>
  )
}
