// 侧栏状态插槽只管理 DOM 宿主；会话数据和交互仍由当前功能页面持有。
import { createContext, useContext, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'

const SidebarStatusContext = createContext<{
  host: HTMLDivElement | null
  setHost: (host: HTMLDivElement | null) => void
} | null>(null)

export function SidebarStatusProvider({ children }: { children: ReactNode }) {
  const [host, setHost] = useState<HTMLDivElement | null>(null)
  return (
    <SidebarStatusContext.Provider value={{ host, setHost }}>
      {children}
    </SidebarStatusContext.Provider>
  )
}

export function SidebarStatusHost() {
  const context = useContext(SidebarStatusContext)
  return (
    <div
      ref={context?.setHost}
      data-testid="sidebar-session-usage"
      className="min-w-0 empty:hidden"
    />
  )
}

export function SidebarStatusPortal({
  enabled,
  children,
}: {
  enabled: boolean
  children: ReactNode
}) {
  const host = useContext(SidebarStatusContext)?.host
  return enabled && host ? createPortal(children, host) : null
}
