// 桌面程序更新控制器：带 shellInstaller 桥接的壳按需加载状态镜像（shell-installer-poller.ts），
// 浏览器、手机和旧壳上什么都不加载。入口包只留这一小段。
import { useEffect, useState } from 'react'
import type { ShellInstallerController } from '@/types/update'

// settled：壳已经回过一次话。之后 controller 仍为 null 说明这个平台不能自更新（例如 Intel Mac）。
export function useShellInstaller(source: unknown): {
  controller: ShellInstallerController | null
  settled: boolean
} {
  const [state, setState] = useState<{
    controller: ShellInstallerController | null
    settled: boolean
  }>({ controller: null, settled: false })
  useEffect(() => {
    if (!source || typeof source !== 'object') return
    let active = true
    let stop = () => {}
    void import('./shell-installer-poller')
      .then(({ startShellInstallerPoller, usableShellInstallerBridge }) => {
        if (!active) return
        if (!usableShellInstallerBridge(source)) {
          setState({ controller: null, settled: true })
          return
        }
        stop = startShellInstallerPoller(source, (controller) => {
          if (active) setState({ controller, settled: true })
        })
      })
      .catch(() => {})
    return () => {
      active = false
      stop()
      setState({ controller: null, settled: false })
    }
  }, [source])
  return state
}
