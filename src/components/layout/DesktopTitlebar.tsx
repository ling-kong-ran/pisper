import {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'

type TitlebarTarget = {
  hosts: Record<string, HTMLDivElement | null>
  setHost: (id: string, host: HTMLDivElement | null) => void
}

const TitlebarContext = createContext<TitlebarTarget | null>(null)

export function DesktopTitlebarProvider({ children }: { children: ReactNode }) {
  const [hosts, setHosts] = useState<Record<string, HTMLDivElement | null>>({})
  const setHost = useCallback((id: string, host: HTMLDivElement | null) => {
    setHosts((current) => {
      if (current[id] === host) return current
      const next = { ...current }
      if (host) next[id] = host
      else delete next[id]
      return next
    })
  }, [])
  const value = useMemo(() => ({ hosts, setHost }), [hosts, setHost])
  return <TitlebarContext.Provider value={value}>{children}</TitlebarContext.Provider>
}

export function DesktopTitlebarHost({
  targetId = 'window',
  inline = false,
}: {
  targetId?: string
  inline?: boolean
}) {
  const setHost = useContext(TitlebarContext)?.setHost
  const ref = useCallback(
    (node: HTMLDivElement | null) => setHost?.(targetId, node),
    [setHost, targetId],
  )
  return (
    <div
      ref={ref}
      className={inline ? 'flex h-full shrink-0 items-center' : 'relative min-w-0 flex-1'}
    />
  )
}

export function DesktopTitlebarPortal({
  enabled,
  targetId = 'window',
  inline = false,
  getAnchor,
  children,
}: {
  enabled: boolean
  targetId?: string
  inline?: boolean
  getAnchor: () => HTMLElement | null
  children: ReactNode
}) {
  const host = useContext(TitlebarContext)?.hosts[targetId]
  const [right, setRight] = useState(0)
  useLayoutEffect(() => {
    if (!enabled || !host || inline) return
    let observedAnchor: HTMLElement | null = null
    // 对齐内容栏右沿；宿主不包含窗口按钮，侧栏收起时自然停在窗口按钮左边。
    const measure = () => {
      const anchor = getAnchor()
      if (anchor !== observedAnchor) {
        if (observedAnchor) observer.unobserve(observedAnchor)
        if (anchor) observer.observe(anchor)
        observedAnchor = anchor
      }
      if (!anchor) return
      setRight(
        Math.max(0, host.getBoundingClientRect().right - anchor.getBoundingClientRect().right),
      )
    }
    const observer = new ResizeObserver(measure)
    // 子组件布局 effect 可能早于父节点 ref；宿主首次通知会在 ref 就绪后补齐观察。
    observer.observe(host)
    measure()
    window.addEventListener('resize', measure)
    return () => {
      observer.disconnect()
      window.removeEventListener('resize', measure)
    }
  }, [enabled, getAnchor, host, inline])

  if (!enabled || !host) return null
  return createPortal(
    <div
      data-desktop-titlebar-actions
      className={
        inline
          ? 'flex h-full items-center [-webkit-app-region:no-drag]'
          : 'absolute inset-y-0 flex items-center [-webkit-app-region:no-drag]'
      }
      style={inline ? undefined : { right }}
    >
      {children}
    </div>,
    host,
  )
}
