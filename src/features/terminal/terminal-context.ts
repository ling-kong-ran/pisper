import { createContext, useContext, useSyncExternalStore } from 'react'
import type { DesktopTerminalProfile } from '@/types/update'
import type { TerminalRecord, TerminalRegistry } from './terminal-registry'

export type TerminalRenderer = {
  attach: (host: HTMLElement, onError?: (message: string) => void) => void
  detach: () => void
  dispose: () => void
}
export type TerminalService = {
  registry: TerminalRegistry
  profiles: () => Promise<DesktopTerminalProfile[]>
  renderer: (key: string, create: () => TerminalRenderer) => TerminalRenderer
}
export const TerminalContext = createContext<TerminalService | null>(null)
const EMPTY: readonly TerminalRecord[] = []
const noopSubscribe = () => () => {}

export function useTerminalService() {
  return useContext(TerminalContext)
}
export function useTerminals(sessionId?: string) {
  const service = useTerminalService()
  return useSyncExternalStore(
    service?.registry.subscribe || noopSubscribe,
    () => service?.registry.snapshot(sessionId) || EMPTY,
  )
}
