import { useEffect, useState, type ReactNode } from 'react'
import type { DesktopTerminalProfile } from '@/types/update'
import { subscribeSessionDeletionUpdates } from '@/features/chat/public-events'
import { createTerminalRegistry, type TerminalTransport } from './terminal-registry'
import { TerminalContext, type TerminalService, type TerminalRenderer } from './terminal-context'

type Bridge = Partial<TerminalTransport> & {
  terminalProfiles?: () => Promise<DesktopTerminalProfile[]>
}
export function TerminalProvider({ bridge, children }: { bridge?: Bridge; children: ReactNode }) {
  const [service, setService] = useState<TerminalService | null>(null)
  useEffect(() => {
    if (
      !bridge?.terminalCreate ||
      !bridge.terminalWrite ||
      !bridge.terminalResize ||
      !bridge.terminalClose ||
      !bridge.terminalProfiles
    ) {
      setService(null)
      return
    }
    const registry = createTerminalRegistry({
      terminalCreate: bridge.terminalCreate,
      terminalWrite: bridge.terminalWrite,
      terminalResize: bridge.terminalResize,
      terminalClose: bridge.terminalClose,
    })
    const renderers = new Map<string, TerminalRenderer>()
    let profiles: Promise<DesktopTerminalProfile[]> | undefined
    const getProfiles = bridge.terminalProfiles
    const current: TerminalService = {
      registry,
      profiles: () =>
        (profiles ||= getProfiles().catch((error) => {
          profiles = undefined
          throw error
        })),
      renderer: (key, create) => {
        if (!renderers.has(key)) renderers.set(key, create())
        return renderers.get(key)!
      },
    }
    setService(current)
    const deleted = subscribeSessionDeletionUpdates(window, ({ deletedIds }) =>
      registry.orphan(deletedIds),
    )
    const dispose = () => {
      for (const renderer of renderers.values()) renderer.dispose()
      renderers.clear()
      // 退出时尝试关闭每个自有进程；单个失败不能阻止其他进程释放。
      void registry.dispose().catch(() => {})
    }
    window.addEventListener('pagehide', dispose)
    return () => {
      deleted()
      window.removeEventListener('pagehide', dispose)
      dispose()
    }
  }, [bridge])
  return <TerminalContext.Provider value={service}>{children}</TerminalContext.Provider>
}
