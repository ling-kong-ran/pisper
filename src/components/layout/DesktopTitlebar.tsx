import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'

type TitlebarTarget = {
  hosts: Record<string, HTMLDivElement | null>
  setHost: (id: string, host: HTMLDivElement | null, previous: HTMLDivElement | null) => void
}

const TitlebarContext = createContext<TitlebarTarget | null>(null)

export function useDesktopTitlebarHost(targetIds: readonly string[]) {
  const hosts = useContext(TitlebarContext)?.hosts
  return targetIds.map((id) => hosts?.[id]).find(Boolean) ?? null
}

export function DesktopTitlebarProvider({ children }: { children: ReactNode }) {
  const [hosts, setHosts] = useState<Record<string, HTMLDivElement | null>>({})
  const setHost = useCallback(
    (id: string, host: HTMLDivElement | null, previous: HTMLDivElement | null) => {
      setHosts((current) => {
        // 分组切换时新宿主可能先挂载；旧宿主卸载只能撤销自己，不能删除新注册。
        if (!host && current[id] !== previous) return current
        if (current[id] === host) return current
        const next = { ...current }
        if (host) next[id] = host
        else delete next[id]
        return next
      })
    },
    [],
  )
  const value = useMemo(() => ({ hosts, setHost }), [hosts, setHost])
  return <TitlebarContext.Provider value={value}>{children}</TitlebarContext.Provider>
}

export function DesktopTitlebarHost({
  targetId = 'window',
  inline = true,
}: {
  targetId?: string
  inline?: boolean
}) {
  const setHost = useContext(TitlebarContext)?.setHost
  const previous = useRef<HTMLDivElement | null>(null)
  const ref = useCallback(
    (node: HTMLDivElement | null) => {
      setHost?.(targetId, node, previous.current)
      previous.current = node
    },
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
  children,
}: {
  enabled: boolean
  targetId?: string
  children: ReactNode
}) {
  const host = useContext(TitlebarContext)?.hosts[targetId]
  if (!enabled || !host) return null
  return createPortal(
    <div
      data-desktop-titlebar-actions
      className="flex h-full items-center [-webkit-app-region:no-drag]"
      onPointerDown={(event) => event.stopPropagation()}
    >
      {children}
    </div>,
    host,
  )
}
