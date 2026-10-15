// 右屏和会话头共用同一组面板状态，避免聊天页再叠一层抽屉。
import { createContext, useContext, useEffect, useState } from 'react'
import {
  resolveMobileShellMode,
  type MobileContextTab,
  type MobileShellMode,
  type MobileShellPane,
} from '@/components/layout/mobile-shell-layout'

export type MobileShellController = {
  active: boolean
  mode: MobileShellMode | 'off'
  pane: MobileShellPane
  tab: MobileContextTab
  setPane: (pane: MobileShellPane) => void
  openContext: (tab: MobileContextTab) => void
  // 壳层只提供 DOM 插槽，业务内容及其 Provider 仍由原页面拥有。
  sessionToolsHost: HTMLDivElement | null
  setSessionToolsHost: (host: HTMLDivElement | null) => void
}

const inactiveShell: MobileShellController = {
  active: false,
  mode: 'off',
  pane: 'chat',
  tab: 'assets',
  setPane: () => {},
  openContext: () => {},
  sessionToolsHost: null,
  setSessionToolsHost: () => {},
}

const MobileShellContext = createContext<MobileShellController>(inactiveShell)

export const MobileShellProvider = MobileShellContext.Provider

export function useMobileShell() {
  return useContext(MobileShellContext)
}

export function useMobileShellMode(enabled: boolean): MobileShellMode | 'off' {
  const [size, setSize] = useState(() => ({
    width: window.innerWidth,
    height: window.innerHeight,
  }))
  useEffect(() => {
    const update = () => setSize({ width: window.innerWidth, height: window.innerHeight })
    update()
    window.addEventListener('resize', update)
    return () => window.removeEventListener('resize', update)
  }, [])
  if (!enabled) return 'off'
  return resolveMobileShellMode(size.width, size.height)
}
